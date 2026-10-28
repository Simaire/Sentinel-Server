//! Connexion à PostgreSQL et schéma.
//!
//! Le schéma est créé et mis à niveau au démarrage : tables du serveur (comptes, appareils,
//! sessions d'administration, journal d'audit) et tables partagées, générées depuis
//! leur définition (`sync::tables`).

use crate::sync::tables::{schema_sql, TABLES};
use anyhow::{Context, Result};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Postgres, Transaction};
use std::str::FromStr;
use std::time::Duration;

const SERVER_SCHEMA: &[&str] = &[
    "CREATE SEQUENCE IF NOT EXISTS sync_seq",
    "CREATE TABLE IF NOT EXISTS users (
        vrc_id TEXT PRIMARY KEY,
        display_name TEXT NOT NULL,
        status TEXT NOT NULL CHECK (status IN ('pending', 'active', 'banned')),
        role TEXT NOT NULL DEFAULT 'moderator' CHECK (role IN ('moderator', 'admin')),
        created_at BIGINT NOT NULL,
        approved_at BIGINT,
        approved_by TEXT,
        last_seen_at BIGINT,
        note TEXT
    )",
    "CREATE INDEX IF NOT EXISTS users_status ON users (status)",
    // Un appareil = une clé Ed25519 générée par l'appli (la clé privée ne quitte jamais le PC).
    // Le jeton de session en cours est renouvelé à chaque connexion signée.
    "CREATE TABLE IF NOT EXISTS devices (
        id BIGSERIAL PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES users (vrc_id) ON DELETE CASCADE,
        public_key TEXT NOT NULL UNIQUE,
        fingerprint TEXT NOT NULL,
        name TEXT,
        status TEXT NOT NULL CHECK (status IN ('pending', 'active', 'revoked')),
        created_at BIGINT NOT NULL,
        approved_at BIGINT,
        approved_by TEXT,
        last_used_at BIGINT NOT NULL,
        token_hash TEXT UNIQUE,
        token_expires_at BIGINT
    )",
    "CREATE INDEX IF NOT EXISTS devices_user ON devices (user_id)",
    "CREATE INDEX IF NOT EXISTS devices_fingerprint ON devices (fingerprint)",
    "CREATE TABLE IF NOT EXISTS admin_codes (
        code_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES users (vrc_id) ON DELETE CASCADE,
        expires_at BIGINT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS admin_sessions (
        id_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES users (vrc_id) ON DELETE CASCADE,
        created_at BIGINT NOT NULL,
        expires_at BIGINT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS audit_log (
        id BIGSERIAL PRIMARY KEY,
        at BIGINT NOT NULL,
        actor TEXT,
        action TEXT NOT NULL,
        target TEXT,
        detail TEXT
    )",
    "CREATE INDEX IF NOT EXISTS audit_log_at ON audit_log (at DESC)",
    "CREATE TABLE IF NOT EXISTS server_state (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    )",
];

/// Verrou consultatif : un seul serveur à la fois met le schéma à niveau.
const SCHEMA_LOCK: i64 = 0x05e1_71e1;

pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool> {
    let options = PgConnectOptions::from_str(url).context("DATABASE_URL invalide")?;
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .connect_with(options)
        .await
        .context("connexion à PostgreSQL impossible")?;
    migrate(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &PgPool) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SCHEMA_LOCK)
        .execute(&mut *tx)
        .await?;
    for statement in SERVER_SCHEMA {
        sqlx::query(statement).execute(&mut *tx).await.with_context(|| statement.to_string())?;
    }
    for table in TABLES {
        for statement in schema_sql(table) {
            sqlx::query(&statement).execute(&mut *tx).await.with_context(|| statement.clone())?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Trace une action dans le journal d'audit (connexions, décisions des admins…).
pub async fn audit(
    executor: &mut Transaction<'_, Postgres>,
    actor: Option<&str>,
    action: &str,
    target: Option<&str>,
    detail: Option<&str>,
) -> Result<()> {
    sqlx::query("INSERT INTO audit_log (at, actor, action, target, detail) VALUES ($1, $2, $3, $4, $5)")
        .bind(crate::util::now_unix())
        .bind(actor)
        .bind(action)
        .bind(target)
        .bind(detail)
        .execute(&mut **executor)
        .await?;
    Ok(())
}

/// Variante hors transaction.
pub async fn audit_now(pool: &PgPool, actor: Option<&str>, action: &str, target: Option<&str>, detail: Option<&str>) {
    let result = sqlx::query("INSERT INTO audit_log (at, actor, action, target, detail) VALUES ($1, $2, $3, $4, $5)")
        .bind(crate::util::now_unix())
        .bind(actor)
        .bind(action)
        .bind(target)
        .bind(detail)
        .execute(pool)
        .await;
    if let Err(e) = result {
        tracing::warn!("journal d'audit : {e}");
    }
}

pub async fn get_state(pool: &PgPool, key: &str) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT value FROM server_state WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await?)
}

pub async fn set_state(pool: &PgPool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO server_state (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Dernier numéro d'écriture (0 si rien n'a encore été écrit).
pub async fn current_head(pool: &PgPool) -> Result<i64> {
    let (last, called): (i64, bool) = sqlx::query_as("SELECT last_value, is_called FROM sync_seq")
        .fetch_one(pool)
        .await?;
    Ok(if called { last } else { 0 })
}
