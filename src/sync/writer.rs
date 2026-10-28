//! Écrivain unique : toutes les écritures dans les tables partagées passent par lui.
//!
//! - Les envois qui arrivent en même temps sont regroupés dans une seule transaction
//!   (bien moins d'allers-retours avec PostgreSQL sous forte charge).
//! - Une seule transaction à la fois : l'ordre des `seq` est celui des validations, donc
//!   un client qui relit « tout ce qui dépasse mon curseur » ne manque jamais rien.
//! - Chaque table est écrite en une requête (`UNNEST`), en plusieurs passes seulement si
//!   la même ligne apparaît plusieurs fois dans le lot.

use super::tables::{bulk_upsert_sql, Kind, Table};
use super::Cell;
use anyhow::Result;
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot, watch};

/// Envois regroupés au plus par transaction.
const MAX_BATCH_JOBS: usize = 64;
const MAX_BATCH_ROWS: usize = 20_000;
/// Envois en attente au plus (au-delà, les clients attendent : contre-pression).
const QUEUE: usize = 256;

pub struct TableRows {
    pub table: &'static Table,
    pub rows: Vec<Vec<Cell>>,
}

struct Job {
    author: String,
    changes: Vec<TableRows>,
    reply: oneshot::Sender<Result<i64, String>>,
}

#[derive(Clone)]
pub struct Writer {
    tx: mpsc::Sender<Job>,
}

impl Writer {
    pub fn spawn(pool: PgPool, head: watch::Sender<i64>) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE);
        tokio::spawn(run(pool, rx, head));
        Self { tx }
    }

    /// Écrit ces lignes au nom de `author`. Renvoie le nouveau numéro d'écriture.
    pub async fn write(&self, author: &str, changes: Vec<TableRows>) -> Result<i64> {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(Job {
                author: author.to_string(),
                changes,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("écrivain arrêté"))?;
        answer
            .await
            .map_err(|_| anyhow::anyhow!("écrivain arrêté"))?
            .map_err(anyhow::Error::msg)
    }
}

fn job_rows(job: &Job) -> usize {
    job.changes.iter().map(|c| c.rows.len()).sum()
}

async fn run(pool: PgPool, mut rx: mpsc::Receiver<Job>, head: watch::Sender<i64>) {
    while let Some(first) = rx.recv().await {
        let mut rows = job_rows(&first);
        let mut jobs = vec![first];
        while jobs.len() < MAX_BATCH_JOBS && rows < MAX_BATCH_ROWS {
            match rx.try_recv() {
                Ok(job) => {
                    rows += job_rows(&job);
                    jobs.push(job);
                }
                Err(_) => break,
            }
        }
        match apply(&pool, &jobs).await {
            Ok(seq) => {
                head.send_if_modified(|current| {
                    let changed = seq > *current;
                    *current = (*current).max(seq);
                    changed
                });
                for job in jobs {
                    let _ = job.reply.send(Ok(seq));
                }
            }
            // Un envoi fautif ne doit pas faire échouer les autres : on les rejoue un par un.
            Err(e) if jobs.len() > 1 => {
                tracing::warn!("lot de {} envois rejeté ({e:#}), nouvel essai un par un", jobs.len());
                for job in jobs {
                    let result = apply(&pool, std::slice::from_ref(&job)).await;
                    if let Ok(seq) = &result {
                        head.send_if_modified(|current| {
                            let changed = *seq > *current;
                            *current = (*current).max(*seq);
                            changed
                        });
                    }
                    let _ = job.reply.send(result.map_err(|e| format!("{e:#}")));
                }
            }
            Err(e) => {
                for job in jobs {
                    let _ = job.reply.send(Err(format!("{e:#}")));
                }
            }
        }
    }
}

async fn apply(pool: &PgPool, jobs: &[Job]) -> Result<i64> {
    let mut tx = pool.begin().await?;
    for job in jobs {
        for change in &job.changes {
            upsert(&mut tx, change.table, &change.rows, &job.author).await?;
        }
    }
    let (last, called): (i64, bool) = sqlx::query_as("SELECT last_value, is_called FROM sync_seq")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(if called { last } else { 0 })
}

/// Répartit les lignes en passes sans clé en double (`ON CONFLICT` ne peut pas toucher
/// deux fois la même ligne dans une requête).
fn passes<'a>(table: &Table, rows: &'a [Vec<Cell>]) -> Vec<Vec<&'a Vec<Cell>>> {
    let keys = table.key_indexes();
    let mut seen: HashMap<Vec<&Cell>, usize> = HashMap::new();
    let mut passes: Vec<Vec<&Vec<Cell>>> = Vec::new();
    for row in rows {
        let key: Vec<&Cell> = keys.iter().map(|&i| &row[i]).collect();
        let pass = seen.entry(key).and_modify(|n| *n += 1).or_insert(0);
        if passes.len() <= *pass {
            passes.push(Vec::new());
        }
        passes[*pass].push(row);
    }
    passes
}

async fn upsert(tx: &mut Transaction<'_, Postgres>, table: &Table, rows: &[Vec<Cell>], author: &str) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let sql = bulk_upsert_sql(table);
    for pass in passes(table, rows) {
        let mut query = sqlx::query(&sql);
        for (i, column) in table.columns.iter().enumerate() {
            query = match column.kind {
                Kind::Text => query.bind(
                    pass.iter()
                        .map(|row| match &row[i] {
                            Cell::Text(v) => v.clone(),
                            Cell::Int(_) => None,
                        })
                        .collect::<Vec<Option<String>>>(),
                ),
                Kind::Int => query.bind(
                    pass.iter()
                        .map(|row| match &row[i] {
                            Cell::Int(v) => *v,
                            Cell::Text(_) => None,
                        })
                        .collect::<Vec<Option<i64>>>(),
                ),
            };
        }
        query.bind(author).execute(&mut **tx).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::tables::table;

    #[test]
    fn duplicates_go_to_later_passes() {
        let votes = table("votes").unwrap();
        let row = |voter: &str, value: i64| {
            vec![
                Cell::Text(Some("player".into())),
                Cell::Text(Some("usr_x".into())),
                Cell::Text(Some(voter.into())),
                Cell::Text(None),
                Cell::Int(Some(value)),
                Cell::Int(Some(1)),
            ]
        };
        let rows = [row("usr_a", 1), row("usr_b", 1), row("usr_a", -1)];
        let split = passes(votes, &rows);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].len(), 2);
        assert_eq!(split[1].len(), 1);
    }
}
