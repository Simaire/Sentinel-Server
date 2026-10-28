//! API de l'interface d'administration (session admin obligatoire).

use super::session::AdminSession;
use crate::db::audit;
use crate::error::{ApiError, ApiResult};
use crate::state::{Metrics, Shared};
use crate::sync::tables::{table, Kind, Merge, TABLES};
use crate::sync::writer::TableRows;
use crate::sync::Cell;
use crate::util::{is_user_id, now_unix};
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::HashSet;

/// Longueur montrée d'un texte dans les listes de données.
const PREVIEW_CHARS: usize = 1_500;
const PAGE: i64 = 100;

type JsonResult = ApiResult<Json<Value>>;

async fn table_counts(state: &Shared) -> ApiResult<Vec<Value>> {
    let names: Vec<&str> = TABLES.iter().map(|t| t.name).collect();
    // Estimation PostgreSQL (instantanée) ; comptage exact pour les petites tables.
    let estimates: Vec<(String, f32)> =
        sqlx::query_as("SELECT relname::text, reltuples FROM pg_class WHERE relkind = 'r' AND relname = ANY($1)")
            .bind(&names)
            .fetch_all(&state.pool)
            .await?;
    let mut out = Vec::new();
    for name in names {
        let estimate = estimates.iter().find(|(n, _)| n == name).map_or(-1.0, |(_, e)| *e);
        let (rows, exact) = if estimate < 200_000.0 {
            let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {name}"))
                .fetch_one(&state.pool)
                .await?;
            (n, true)
        } else {
            (estimate as i64, false)
        };
        out.push(json!({ "name": name, "rows": rows, "exact": exact }));
    }
    Ok(out)
}

pub async fn overview(State(state): State<Shared>, _admin: AdminSession) -> JsonResult {
    let users: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT status, role, count(*) FROM users GROUP BY status, role")
            .fetch_all(&state.pool)
            .await?;
    let count = |status: &str| users.iter().filter(|(s, _, _)| s == status).map(|(_, _, n)| n).sum::<i64>();
    let admins: i64 = users
        .iter()
        .filter(|(s, r, _)| s == "active" && r == "admin")
        .map(|(_, _, n)| n)
        .sum();
    let (devices, pending_devices): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'active'), count(*) FILTER (WHERE status = 'pending') FROM devices",
    )
    .fetch_one(&state.pool)
    .await?;
    let db_size: i64 = sqlx::query_scalar("SELECT pg_database_size(current_database())")
        .fetch_one(&state.pool)
        .await?;
    let m = &state.metrics;
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": state.started.elapsed().as_secs(),
        "head": *state.head.borrow(),
        "users": { "pending": count("pending"), "active": count("active"), "banned": count("banned"), "admins": admins },
        "devices": devices,
        "pending_devices": pending_devices,
        "online": state.presence.online(),
        "realtime_clients": Metrics::get(&m.waiting),
        "db_size_bytes": db_size,
        "tables": table_counts(&state).await?,
        "metrics": {
            "pushes": Metrics::get(&m.pushes),
            "rows_written": Metrics::get(&m.rows_written),
            "pulls": Metrics::get(&m.pulls),
            "rows_read": Metrics::get(&m.rows_read),
            "logins": Metrics::get(&m.logins),
            "login_failures": Metrics::get(&m.login_failures),
        },
        "registration_open": registration_open(&state).await?,
    })))
}

async fn registration_open(state: &Shared) -> ApiResult<bool> {
    Ok(crate::db::get_state(&state.pool, "registration_open")
        .await?
        .is_none_or(|v| v != "false"))
}

#[derive(Deserialize)]
pub struct UsersQuery {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub q: String,
}

#[derive(sqlx::FromRow, Serialize)]
struct UserRow {
    vrc_id: String,
    display_name: String,
    status: String,
    role: String,
    created_at: i64,
    approved_at: Option<i64>,
    approved_by: Option<String>,
    last_seen_at: Option<i64>,
    note: Option<String>,
    /// Appareils validés et appareils en attente.
    devices: i64,
    pending_devices: i64,
}

const DEVICE_COUNTS: &str = "(SELECT count(*) FROM devices d WHERE d.user_id = u.vrc_id AND d.status = 'active') AS devices,
    (SELECT count(*) FROM devices d WHERE d.user_id = u.vrc_id AND d.status = 'pending') AS pending_devices";

pub async fn users(State(state): State<Shared>, _admin: AdminSession, Query(query): Query<UsersQuery>) -> JsonResult {
    let needle = format!("%{}%", query.q.trim().replace(['%', '_'], ""));
    // « pending » : comptes en attente et comptes actifs avec un nouvel appareil à valider.
    let rows: Vec<UserRow> = sqlx::query_as(&format!(
        "SELECT * FROM (
            SELECT u.vrc_id, u.display_name, u.status, u.role, u.created_at, u.approved_at, u.approved_by,
                   u.last_seen_at, u.note, {DEVICE_COUNTS}
            FROM users u
            WHERE u.display_name ILIKE $2 OR u.vrc_id = $3
         ) x
         WHERE $1 = '' OR x.status = $1 OR ($1 = 'pending' AND x.status = 'active' AND x.pending_devices > 0)
         ORDER BY CASE WHEN x.status = 'pending' OR (x.status = 'active' AND x.pending_devices > 0) THEN 0
                       WHEN x.status = 'active' THEN 1 ELSE 2 END,
                  x.last_seen_at DESC NULLS LAST
         LIMIT 1000"
    ))
    .bind(&query.status)
    .bind(&needle)
    .bind(query.q.trim())
    .fetch_all(&state.pool)
    .await?;
    let online: HashSet<String> = state.presence.online().into_iter().map(|m| m.user_id).collect();
    let list: Vec<Value> = rows
        .into_iter()
        .map(|u| {
            let is_online = online.contains(&u.vrc_id);
            let mut v = serde_json::to_value(u).unwrap_or_default();
            v["online"] = Value::Bool(is_online);
            v
        })
        .collect();
    Ok(Json(json!({ "users": list })))
}

fn check_user_id(id: &str) -> ApiResult<()> {
    is_user_id(id)
        .then_some(())
        .ok_or_else(|| ApiError::BadRequest("Identifiant invalide.".into()))
}

pub async fn user_detail(State(state): State<Shared>, _admin: AdminSession, Path(id): Path<String>) -> JsonResult {
    check_user_id(&id)?;
    let user: Option<UserRow> = sqlx::query_as(&format!(
        "SELECT u.vrc_id, u.display_name, u.status, u.role, u.created_at, u.approved_at, u.approved_by,
                u.last_seen_at, u.note, {DEVICE_COUNTS}
         FROM users u WHERE u.vrc_id = $1"
    ))
    .bind(&id)
    .fetch_optional(&state.pool)
    .await?;
    let user = user.ok_or(ApiError::NotFound)?;
    let devices: Vec<DeviceRow> = sqlx::query_as(
        "SELECT id, fingerprint, name, status, created_at, approved_at, approved_by, last_used_at FROM devices
         WHERE user_id = $1
         ORDER BY CASE status WHEN 'pending' THEN 0 WHEN 'active' THEN 1 ELSE 2 END, last_used_at DESC
         LIMIT 50",
    )
    .bind(&id)
    .fetch_all(&state.pool)
    .await?;
    let audit_rows: Vec<(i64, Option<String>, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT at, actor, action, target, detail FROM audit_log WHERE actor = $1 OR target = $1
         ORDER BY at DESC LIMIT 50",
    )
    .bind(&id)
    .fetch_all(&state.pool)
    .await?;
    let contributions: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM reports WHERE reporter_id = $1 AND deleted = 0),
                (SELECT count(*) FROM votes WHERE voter_id = $1 AND value <> 0),
                (SELECT count(*) FROM moderation_actions WHERE actor_id = $1)",
    )
    .bind(&id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(json!({
        "user": user,
        "online": state.presence.online().into_iter().find(|m| m.user_id == id),
        "devices": devices,
        "audit": audit_rows.iter().map(|(at, actor, action, target, detail)| json!({
            "at": at, "actor": actor, "action": action, "target": target, "detail": detail,
        })).collect::<Vec<_>>(),
        "contributions": { "reports": contributions.0, "votes": contributions.1, "actions": contributions.2 },
    })))
}

#[derive(sqlx::FromRow, Serialize)]
struct DeviceRow {
    id: i64,
    fingerprint: String,
    name: Option<String>,
    status: String,
    created_at: i64,
    approved_at: Option<i64>,
    approved_by: Option<String>,
    last_used_at: i64,
}

#[derive(Deserialize)]
pub struct StatusBody {
    pub status: String,
}

pub async fn set_status(State(state): State<Shared>, admin: AdminSession, Path(id): Path<String>, Json(body): Json<StatusBody>) -> JsonResult {
    check_user_id(&id)?;
    if !matches!(body.status.as_str(), "active" | "pending" | "banned") {
        return Err(ApiError::BadRequest("Statut inconnu.".into()));
    }
    if id == admin.user_id {
        return Err(ApiError::BadRequest("Tu ne peux pas changer ton propre statut.".into()));
    }
    let now = now_unix();
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query(
        "UPDATE users SET status = $2,
            approved_at = CASE WHEN $2 = 'active' THEN $3 ELSE approved_at END,
            approved_by = CASE WHEN $2 = 'active' THEN $4 ELSE approved_by END
         WHERE vrc_id = $1",
    )
    .bind(&id)
    .bind(&body.status)
    .bind(now)
    .bind(&admin.user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ApiError::NotFound);
    }
    // Valider un compte valide aussi ses appareils en attente (codes visibles dans la fiche).
    if body.status == "active" {
        sqlx::query(
            "UPDATE devices SET status = 'active', approved_at = $2, approved_by = $3
             WHERE user_id = $1 AND status = 'pending'",
        )
        .bind(&id)
        .bind(now)
        .bind(&admin.user_id)
        .execute(&mut *tx)
        .await?;
    }
    if body.status == "banned" {
        sqlx::query("UPDATE devices SET status = 'revoked', token_hash = NULL WHERE user_id = $1")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM admin_sessions WHERE user_id = $1")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    audit(&mut tx, Some(&admin.user_id), &format!("user.{}", body.status), Some(&id), None).await?;
    tx.commit().await?;
    state.tokens.forget_user(&id);
    if body.status != "active" {
        state.presence.remove(&id);
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct RoleBody {
    pub role: String,
}

pub async fn set_role(State(state): State<Shared>, admin: AdminSession, Path(id): Path<String>, Json(body): Json<RoleBody>) -> JsonResult {
    check_user_id(&id)?;
    if !matches!(body.role.as_str(), "moderator" | "admin") {
        return Err(ApiError::BadRequest("Rôle inconnu.".into()));
    }
    if id == admin.user_id {
        return Err(ApiError::BadRequest("Tu ne peux pas changer ton propre rôle.".into()));
    }
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query("UPDATE users SET role = $2 WHERE vrc_id = $1")
        .bind(&id)
        .bind(&body.role)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(ApiError::NotFound);
    }
    if body.role != "admin" {
        sqlx::query("DELETE FROM admin_sessions WHERE user_id = $1")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    audit(&mut tx, Some(&admin.user_id), &format!("user.role.{}", body.role), Some(&id), None).await?;
    tx.commit().await?;
    state.tokens.forget_user(&id);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct NoteBody {
    pub note: String,
}

pub async fn set_note(State(state): State<Shared>, admin: AdminSession, Path(id): Path<String>, Json(body): Json<NoteBody>) -> JsonResult {
    check_user_id(&id)?;
    let note: String = body.note.chars().take(1_000).collect();
    sqlx::query("UPDATE users SET note = NULLIF($2, '') WHERE vrc_id = $1")
        .bind(&id)
        .bind(&note)
        .execute(&state.pool)
        .await?;
    crate::db::audit_now(&state.pool, Some(&admin.user_id), "user.note", Some(&id), None).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn revoke_user_devices(State(state): State<Shared>, admin: AdminSession, Path(id): Path<String>) -> JsonResult {
    check_user_id(&id)?;
    let revoked = sqlx::query(
        "UPDATE devices SET status = 'revoked', token_hash = NULL WHERE user_id = $1 AND status <> 'revoked'",
    )
    .bind(&id)
    .execute(&state.pool)
    .await?
    .rows_affected();
    state.tokens.forget_user(&id);
    state.presence.remove(&id);
    crate::db::audit_now(&state.pool, Some(&admin.user_id), "devices.revoke_all", Some(&id), Some(&revoked.to_string())).await;
    Ok(Json(json!({ "revoked": revoked })))
}

/// Valide un appareil (et son compte s'il était en attente), après comparaison du code.
pub async fn approve_device(State(state): State<Shared>, admin: AdminSession, Path(device_id): Path<i64>) -> JsonResult {
    let now = now_unix();
    let mut tx = state.pool.begin().await?;
    let found: Option<(String, String)> = sqlx::query_as(
        "UPDATE devices SET status = 'active', approved_at = $2, approved_by = $3
         WHERE id = $1 AND status = 'pending' RETURNING user_id, fingerprint",
    )
    .bind(device_id)
    .bind(now)
    .bind(&admin.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (user, code) = found.ok_or(ApiError::NotFound)?;
    sqlx::query(
        "UPDATE users SET status = 'active', approved_at = $2, approved_by = $3 WHERE vrc_id = $1 AND status = 'pending'",
    )
    .bind(&user)
    .bind(now)
    .bind(&admin.user_id)
    .execute(&mut *tx)
    .await?;
    audit(&mut tx, Some(&admin.user_id), "device.approve", Some(&user), Some(&code)).await?;
    tx.commit().await?;
    state.tokens.forget_user(&user);
    Ok(Json(json!({ "ok": true })))
}

pub async fn revoke_device(State(state): State<Shared>, admin: AdminSession, Path(device_id): Path<i64>) -> JsonResult {
    let found: Option<(String, String)> = sqlx::query_as(
        "UPDATE devices SET status = 'revoked', token_hash = NULL WHERE id = $1 AND status <> 'revoked'
         RETURNING user_id, fingerprint",
    )
    .bind(device_id)
    .fetch_optional(&state.pool)
    .await?;
    let (user, code) = found.ok_or(ApiError::NotFound)?;
    state.tokens.forget_user(&user);
    state.presence.remove(&user);
    crate::db::audit_now(&state.pool, Some(&admin.user_id), "device.revoke", Some(&user), Some(&code)).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_user(State(state): State<Shared>, admin: AdminSession, Path(id): Path<String>) -> JsonResult {
    check_user_id(&id)?;
    if id == admin.user_id {
        return Err(ApiError::BadRequest("Tu ne peux pas supprimer ton propre compte.".into()));
    }
    let mut tx = state.pool.begin().await?;
    let deleted = sqlx::query("DELETE FROM users WHERE vrc_id = $1")
        .bind(&id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(ApiError::NotFound);
    }
    audit(&mut tx, Some(&admin.user_id), "user.delete", Some(&id), None).await?;
    tx.commit().await?;
    state.tokens.forget_user(&id);
    state.presence.remove(&id);
    Ok(Json(json!({ "ok": true })))
}

pub async fn tables(State(state): State<Shared>, _admin: AdminSession) -> JsonResult {
    let counts = table_counts(&state).await?;
    let list: Vec<Value> = TABLES
        .iter()
        .zip(counts)
        .map(|(t, count)| {
            json!({
                "name": t.name,
                "columns": t.column_names(),
                "keys": t.keys(),
                "rows": count["rows"],
                "exact": count["exact"],
            })
        })
        .collect();
    Ok(Json(json!({ "tables": list })))
}

#[derive(Deserialize)]
pub struct RowsQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub offset: i64,
}

/// Lignes d'une table, les plus récentes d'abord, avec recherche sur les colonnes clés
/// et les noms.
pub async fn table_rows(State(state): State<Shared>, _admin: AdminSession, Path(name): Path<String>, Query(query): Query<RowsQuery>) -> JsonResult {
    let t = table(&name).ok_or(ApiError::NotFound)?;
    let searchable: Vec<&str> = t
        .columns
        .iter()
        .filter(|c| {
            c.kind == Kind::Text
                && (c.merge == Merge::Key || c.name.contains("name") || c.name == "title" || c.name.ends_with("_id"))
        })
        .map(|c| c.name)
        .collect();
    let filter = if query.q.trim().is_empty() || searchable.is_empty() {
        "TRUE".to_string()
    } else {
        searchable
            .iter()
            .map(|c| format!("{c} ILIKE $3"))
            .collect::<Vec<_>>()
            .join(" OR ")
    };
    let sql = format!(
        "SELECT {}, seq, updated_by FROM {} WHERE {filter} ORDER BY seq DESC LIMIT $1 OFFSET $2",
        t.column_names().join(", "),
        t.name
    );
    let needle = format!("%{}%", query.q.trim().replace(['%', '_'], ""));
    let rows = sqlx::query(&sql)
        .bind(PAGE)
        .bind(query.offset.max(0))
        .bind(&needle)
        .fetch_all(&state.pool)
        .await?;
    let n = t.columns.len();
    let list: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut values: Vec<Value> = t
                .columns
                .iter()
                .enumerate()
                .map(|(i, c)| match c.kind {
                    Kind::Text => row
                        .try_get::<Option<String>, _>(i)
                        .ok()
                        .flatten()
                        .map_or(Value::Null, |s| {
                            if s.chars().count() > PREVIEW_CHARS {
                                Value::String(format!("{}…", s.chars().take(PREVIEW_CHARS).collect::<String>()))
                            } else {
                                Value::String(s)
                            }
                        }),
                    Kind::Int => row.try_get::<Option<i64>, _>(i).ok().flatten().map_or(Value::Null, Value::from),
                })
                .collect();
            values.push(row.try_get::<i64, _>(n).map_or(Value::Null, Value::from));
            values.push(row.try_get::<Option<String>, _>(n + 1).ok().flatten().map_or(Value::Null, Value::String));
            Value::Array(values)
        })
        .collect();
    let mut columns: Vec<&str> = t.column_names();
    columns.push("seq");
    columns.push("updated_by");
    Ok(Json(json!({ "columns": columns, "rows": list, "page_size": PAGE })))
}

/// Retire un signalement (il reste marqué supprimé pour atteindre toutes les applis).
pub async fn delete_report(State(state): State<Shared>, admin: AdminSession, Path(id): Path<String>) -> JsonResult {
    let reports = table("reports").expect("table reports");
    if id.is_empty() || id.len() > 512 {
        return Err(ApiError::BadRequest("Identifiant invalide.".into()));
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM reports WHERE id = $1)")
        .bind(&id)
        .fetch_one(&state.pool)
        .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let now = now_unix();
    let row: Vec<Cell> = reports
        .columns
        .iter()
        .map(|c| match c.name {
            "id" => Cell::Text(Some(id.clone())),
            "deleted" => Cell::Int(Some(1)),
            "updated_at" => Cell::Int(Some(now)),
            _ => match c.kind {
                Kind::Text => Cell::Text(None),
                Kind::Int => Cell::Int(None),
            },
        })
        .collect();
    state
        .writer
        .write(&admin.user_id, vec![TableRows { table: reports, rows: vec![row] }])
        .await?;
    crate::db::audit_now(&state.pool, Some(&admin.user_id), "report.delete", Some(&id), None).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub q: String,
}

pub async fn audit_log(State(state): State<Shared>, _admin: AdminSession, Query(query): Query<AuditQuery>) -> JsonResult {
    let needle = format!("%{}%", query.q.trim().replace(['%', '_'], ""));
    let rows: Vec<(i64, i64, Option<String>, String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT a.id, a.at, a.actor, a.action, a.target, a.detail, u.display_name
         FROM audit_log a LEFT JOIN users u ON u.vrc_id = a.actor
         WHERE $3 = '%%' OR a.action ILIKE $3 OR a.actor ILIKE $3 OR a.target ILIKE $3 OR u.display_name ILIKE $3
         ORDER BY a.id DESC LIMIT $1 OFFSET $2",
    )
    .bind(PAGE)
    .bind(query.offset.max(0))
    .bind(&needle)
    .fetch_all(&state.pool)
    .await?;
    let list: Vec<Value> = rows
        .into_iter()
        .map(|(id, at, actor, action, target, detail, actor_name)| {
            json!({ "id": id, "at": at, "actor": actor, "actor_name": actor_name, "action": action, "target": target, "detail": detail })
        })
        .collect();
    Ok(Json(json!({ "entries": list, "page_size": PAGE })))
}

#[derive(Deserialize)]
pub struct SettingsBody {
    pub registration_open: bool,
}

pub async fn get_settings(State(state): State<Shared>, _admin: AdminSession) -> JsonResult {
    Ok(Json(json!({
        "registration_open": registration_open(&state).await?,
        "public_url": state.cfg.public_url,
        "token_ttl_days": state.cfg.token_ttl_days,
    })))
}

pub async fn set_settings(State(state): State<Shared>, admin: AdminSession, Json(body): Json<SettingsBody>) -> JsonResult {
    crate::db::set_state(&state.pool, "registration_open", if body.registration_open { "true" } else { "false" }).await?;
    crate::db::audit_now(
        &state.pool,
        Some(&admin.user_id),
        "settings.registration",
        None,
        Some(if body.registration_open { "ouvertes" } else { "fermées" }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}
