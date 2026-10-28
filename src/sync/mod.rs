//! Synchronisation des applis Sentinel.
//!
//! - `POST /api/v1/sync/push` : l'appli envoie ses modifications, vérifiées puis écrites.
//! - `POST /api/v1/sync/pull` : l'appli relit ce qui a changé depuis ses curseurs.
//! - `GET  /api/v1/sync/wait?since=N` : attente longue, répond dès qu'une écriture dépasse N
//!   (au plus 25 s) : l'appli ne relit que lorsque quelque chose a changé.

pub mod tables;
pub mod writer;

use crate::auth::ActiveUser;
use crate::error::{ApiError, ApiResult};
use crate::state::Shared;
use crate::util::now_unix;
use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::Row;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tables::{pull_sql, Kind, Merge, Table, TABLES};
use writer::TableRows;

/// Lignes acceptées au plus par envoi.
const MAX_PUSH_ROWS: usize = 5_000;
/// Lignes renvoyées au plus par table et par relecture.
const MAX_PULL_ROWS: i64 = 2_000;
/// Durée maximale d'une attente longue.
const WAIT_SECS: u64 = 25;
/// Une date de modification dans le futur ne doit pas gagner toutes les fusions.
const CLOCK_SLACK_SECS: i64 = 60;

/// Valeur d'une colonne, vérifiée.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Cell {
    Text(Option<String>),
    Int(Option<i64>),
}

impl Cell {
    fn to_json(&self) -> Value {
        match self {
            Cell::Text(Some(v)) => Value::String(v.clone()),
            Cell::Int(Some(v)) => Value::from(*v),
            Cell::Text(None) | Cell::Int(None) => Value::Null,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Cell::Text(v) => v.as_deref(),
            Cell::Int(_) => None,
        }
    }
}

#[derive(Deserialize)]
pub struct PushTable {
    /// Colonnes dans l'ordre des valeurs : doit correspondre à la définition du serveur.
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

#[derive(Deserialize)]
pub struct PushBody {
    pub tables: HashMap<String, PushTable>,
}

#[derive(Serialize)]
pub struct PushResult {
    pub accepted: usize,
    pub rejected: usize,
    /// Premières raisons de refus, pour le diagnostic.
    pub reasons: Vec<String>,
    pub head: i64,
}

/// Qui écrit : compte vérifié et droits.
pub struct Author<'a> {
    pub id: &'a str,
    pub display_name: &'a str,
    pub admin: bool,
}

fn parse_cell(kind: Kind, max_len: usize, value: &Value) -> Result<Cell, &'static str> {
    match (kind, value) {
        (Kind::Text, Value::Null) => Ok(Cell::Text(None)),
        (Kind::Text, Value::String(s)) if s.len() <= max_len => Ok(Cell::Text(Some(s.clone()))),
        (Kind::Text, Value::String(_)) => Err("texte trop long"),
        (Kind::Int, Value::Null) => Ok(Cell::Int(None)),
        (Kind::Int, Value::Number(n)) => n.as_i64().map(|v| Cell::Int(Some(v))).ok_or("nombre invalide"),
        _ => Err("type de valeur inattendu"),
    }
}

/// Règles propres à certaines tables (valeurs autorisées).
fn check_row(table: &Table, row: &[Cell]) -> Result<(), &'static str> {
    let text = |name: &str| table.index_of(name).and_then(|i| row[i].as_text());
    let int = |name: &str| match table.index_of(name).map(|i| &row[i]) {
        Some(Cell::Int(v)) => *v,
        _ => None,
    };
    match table.name {
        "votes" => {
            if !matches!(text("target_type"), Some("player" | "avatar" | "group")) {
                return Err("type de vote inconnu");
            }
            if !matches!(int("value"), Some(-1..=1)) {
                return Err("valeur de vote invalide");
            }
        }
        "reports" if !matches!(int("deleted"), None | Some(0 | 1)) => return Err("suppression invalide"),
        _ => {}
    }
    Ok(())
}

/// Vérifie et prépare les lignes d'une table. Renvoie les lignes acceptées et les refus.
pub fn validate(table: &'static Table, body: &PushTable, author: &Author<'_>, now: i64) -> ApiResult<(Vec<Vec<Cell>>, Vec<String>)> {
    let expected = table.column_names();
    if body.columns.len() != expected.len() || body.columns.iter().zip(&expected).any(|(a, b)| a != b) {
        return Err(ApiError::BadRequest(format!(
            "Colonnes de « {} » différentes de celles du serveur : mets Sentinel à jour.",
            table.name
        )));
    }
    let updated_at = table.index_of("updated_at");
    let owner = table.owner.as_ref().map(|o| {
        (
            o,
            table.index_of(o.id_column).expect("colonne de l'auteur"),
            o.name_column.and_then(|n| table.index_of(n)),
        )
    });
    let mut rows = Vec::with_capacity(body.rows.len());
    let mut reasons = Vec::new();
    'rows: for raw in &body.rows {
        if raw.len() != table.columns.len() {
            reasons.push(format!("{} : nombre de valeurs incorrect", table.name));
            continue;
        }
        let mut row = Vec::with_capacity(raw.len());
        for (column, value) in table.columns.iter().zip(raw) {
            match parse_cell(column.kind, column.max_len, value) {
                Ok(cell) => {
                    if column.merge == Merge::Key && !matches!(&cell, Cell::Text(Some(s)) if !s.is_empty()) {
                        reasons.push(format!("{}.{} : clé vide", table.name, column.name));
                        continue 'rows;
                    }
                    row.push(cell);
                }
                Err(why) => {
                    reasons.push(format!("{}.{} : {why}", table.name, column.name));
                    continue 'rows;
                }
            }
        }
        if let Err(why) = check_row(table, &row) {
            reasons.push(format!("{} : {why}", table.name));
            continue;
        }
        // Date de modification bornée : une horloge en avance ne gagne pas toutes les fusions.
        if let Some(i) = updated_at {
            if let Cell::Int(Some(v)) = &mut row[i] {
                *v = (*v).min(now + CLOCK_SLACK_SECS);
            }
        }
        // L'auteur d'une ligne est toujours le compte connecté, avec son pseudo vérifié.
        if let Some((owner, id_index, name_index)) = &owner {
            match row[*id_index].as_text() {
                Some(id) if id == author.id => {}
                None if owner.optional => {}
                _ => {
                    reasons.push(format!("{} : écrite au nom d'un autre compte", table.name));
                    continue;
                }
            }
            if let (Some(i), Some(_)) = (name_index, row[*id_index].as_text()) {
                row[*i] = Cell::Text(Some(author.display_name.to_string()));
            }
        }
        rows.push(row);
    }
    Ok((rows, reasons))
}

/// Retire les lignes qui modifieraient l'écrit d'un autre auteur (signalements, alertes,
/// actions de modération). Un admin peut supprimer un signalement.
async fn protect_existing(state: &Shared, table: &Table, rows: &mut Vec<Vec<Cell>>, author: &Author<'_>, reasons: &mut Vec<String>) -> ApiResult<()> {
    let Some(owner) = table.owner.as_ref().filter(|o| o.protect_existing) else {
        return Ok(());
    };
    let keys = table.key_indexes();
    if keys.len() != 1 || rows.is_empty() {
        return Ok(());
    }
    let key = keys[0];
    let ids: Vec<String> = rows.iter().filter_map(|r| r[key].as_text().map(str::to_string)).collect();
    let sql = format!(
        "SELECT {k} FROM {t} WHERE {k} = ANY($1) AND {o} IS DISTINCT FROM $2",
        k = table.columns[key].name,
        t = table.name,
        o = owner.id_column
    );
    let foreign: HashSet<String> = sqlx::query_scalar(&sql)
        .bind(&ids)
        .bind(author.id)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .collect();
    if foreign.is_empty() {
        return Ok(());
    }
    let before = rows.len();
    rows.retain(|r| {
        let theirs = r[key].as_text().is_some_and(|id| foreign.contains(id));
        !theirs || (author.admin && table.name == "reports")
    });
    if rows.len() < before {
        reasons.push(format!("{} : {} ligne(s) d'un autre auteur", table.name, before - rows.len()));
    }
    Ok(())
}

pub async fn push(State(state): State<Shared>, user: ActiveUser, Json(body): Json<PushBody>) -> ApiResult<Json<PushResult>> {
    if !state.limits.sync.allow(&user.id) {
        return Err(ApiError::TooManyRequests);
    }
    let total: usize = body.tables.values().map(|t| t.rows.len()).sum();
    if total > MAX_PUSH_ROWS {
        return Err(ApiError::BadRequest(format!("{MAX_PUSH_ROWS} lignes au plus par envoi.")));
    }
    let author = Author {
        id: &user.id,
        display_name: &user.display_name,
        admin: user.is_admin(),
    };
    let now = now_unix();
    let mut changes = Vec::new();
    let mut reasons = Vec::new();
    let mut accepted = 0;
    // Ordre fixe des tables : toujours le même ordre d'écriture.
    for table in TABLES {
        let Some(part) = body.tables.get(table.name) else {
            continue;
        };
        let (mut rows, mut why) = validate(table, part, &author, now)?;
        protect_existing(&state, table, &mut rows, &author, &mut why).await?;
        accepted += rows.len();
        reasons.append(&mut why);
        if !rows.is_empty() {
            changes.push(TableRows { table, rows });
        }
    }
    if let Some(unknown) = body.tables.keys().find(|name| tables::table(name).is_none()) {
        reasons.push(format!("table inconnue : {unknown}"));
    }
    let head = if changes.is_empty() {
        *state.head.borrow()
    } else {
        state.writer.write(&user.id, changes).await?
    };
    state.metrics.pushes.fetch_add(1, Ordering::Relaxed);
    state.metrics.rows_written.fetch_add(accepted as u64, Ordering::Relaxed);
    let rejected = total - accepted;
    reasons.truncate(10);
    Ok(Json(PushResult {
        accepted,
        rejected,
        reasons,
        head,
    }))
}

#[derive(Deserialize)]
pub struct PullBody {
    /// Dernier `seq` déjà reçu, par table.
    #[serde(default)]
    pub cursors: HashMap<String, i64>,
}

#[derive(Serialize)]
pub struct PulledTable {
    pub columns: Vec<&'static str>,
    pub rows: Vec<Vec<Value>>,
    /// `seq` de chaque ligne, dans le même ordre.
    pub seqs: Vec<i64>,
}

#[derive(Serialize)]
pub struct PullResult {
    pub head: i64,
    /// Il reste des lignes : relire avec les nouveaux curseurs.
    pub more: bool,
    pub tables: BTreeMap<&'static str, PulledTable>,
}

fn read_cell(row: &sqlx::postgres::PgRow, index: usize, kind: Kind) -> Cell {
    match kind {
        Kind::Text => Cell::Text(row.try_get::<Option<String>, _>(index).ok().flatten()),
        Kind::Int => Cell::Int(row.try_get::<Option<i64>, _>(index).ok().flatten()),
    }
}

pub async fn pull(State(state): State<Shared>, user: ActiveUser, Json(body): Json<PullBody>) -> ApiResult<Json<PullResult>> {
    if !state.limits.sync.allow(&user.id) {
        return Err(ApiError::TooManyRequests);
    }
    let head = *state.head.borrow();
    let mut tables = BTreeMap::new();
    let mut more = false;
    let mut read = 0u64;
    for table in TABLES {
        let cursor = body.cursors.get(table.name).copied().unwrap_or(0);
        if cursor >= head {
            continue;
        }
        let rows = sqlx::query(&pull_sql(table))
            .bind(cursor)
            .bind(MAX_PULL_ROWS)
            .fetch_all(&state.pool)
            .await?;
        if rows.is_empty() {
            continue;
        }
        more |= rows.len() as i64 == MAX_PULL_ROWS;
        read += rows.len() as u64;
        let seq_index = table.columns.len();
        let mut pulled = PulledTable {
            columns: table.column_names(),
            rows: Vec::with_capacity(rows.len()),
            seqs: Vec::with_capacity(rows.len()),
        };
        for row in &rows {
            pulled.rows.push(
                table
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, c)| read_cell(row, i, c.kind).to_json())
                    .collect(),
            );
            pulled.seqs.push(row.try_get(seq_index)?);
        }
        tables.insert(table.name, pulled);
    }
    state.metrics.pulls.fetch_add(1, Ordering::Relaxed);
    state.metrics.rows_read.fetch_add(read, Ordering::Relaxed);
    Ok(Json(PullResult { head, more, tables }))
}

#[derive(Deserialize)]
pub struct WaitQuery {
    #[serde(default)]
    pub since: i64,
}

/// Répond dès qu'une écriture dépasse `since`, sinon au bout de 25 s.
pub async fn wait(State(state): State<Shared>, _user: ActiveUser, Query(query): Query<WaitQuery>) -> ApiResult<Json<Map<String, Value>>> {
    let mut head = state.head.clone();
    state.metrics.waiting.fetch_add(1, Ordering::Relaxed);
    let changed = tokio::time::timeout(Duration::from_secs(WAIT_SECS), head.wait_for(|h| *h > query.since)).await;
    state.metrics.waiting.fetch_sub(1, Ordering::Relaxed);
    let current = match changed {
        Ok(Ok(h)) => *h,
        _ => *state.head.borrow(),
    };
    let mut out = Map::new();
    out.insert("head".into(), Value::from(current));
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn author() -> Author<'static> {
        Author {
            id: "usr_me",
            display_name: "Moi",
            admin: false,
        }
    }

    fn votes_body(rows: Vec<Vec<Value>>) -> PushTable {
        PushTable {
            columns: tables::table("votes").unwrap().column_names().iter().map(|s| s.to_string()).collect(),
            rows,
        }
    }

    #[test]
    fn validation_enforces_author_types_and_clock() {
        let votes = tables::table("votes").unwrap();
        let body = votes_body(vec![
            json!(["player", "usr_x", "usr_me", "faux pseudo", 1, 9_999_999_999i64]).as_array().unwrap().clone(),
            json!(["player", "usr_x", "usr_autre", null, 1, 1]).as_array().unwrap().clone(),
            json!(["player", "usr_x", "usr_me", null, 7, 1]).as_array().unwrap().clone(),
            json!(["bidon", "usr_x", "usr_me", null, 1, 1]).as_array().unwrap().clone(),
            json!(["player", "", "usr_me", null, 1, 1]).as_array().unwrap().clone(),
            json!(["player", "usr_x", "usr_me", null, "1", 1]).as_array().unwrap().clone(),
        ]);
        let (rows, reasons) = validate(votes, &body, &author(), 1_000).unwrap();
        assert_eq!(rows.len(), 1, "{reasons:?}");
        assert_eq!(reasons.len(), 5);
        assert_eq!(rows[0][3], Cell::Text(Some("Moi".into())), "pseudo réécrit");
        assert_eq!(rows[0][5], Cell::Int(Some(1_060)), "date bornée");
    }

    #[test]
    fn wrong_columns_are_refused() {
        let votes = tables::table("votes").unwrap();
        let body = PushTable {
            columns: vec!["target_type".into()],
            rows: vec![],
        };
        assert!(validate(votes, &body, &author(), 0).is_err());
    }
}
