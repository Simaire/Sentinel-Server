//! Jetons de session des applis : 256 bits aléatoires, un par appareil, renouvelés à chaque
//! connexion signée. Seule leur empreinte SHA-256 est stockée ; un cache de 30 s évite une
//! requête à chaque appel.

use crate::error::ApiResult;
use crate::util::{now_unix, random_token, token_hash};
use dashmap::DashMap;
use sqlx::{PgPool, Postgres, Transaction};
use std::time::{Duration, Instant};

pub const TOKEN_PREFIX: &str = "sst_";
const CACHE_TTL: Duration = Duration::from_secs(30);
/// La date d'expiration glissante n'est réécrite qu'une fois par heure au plus.
const TOUCH_EVERY_SECS: i64 = 3_600;

#[derive(Clone, Debug)]
pub struct AuthUser {
    pub id: String,
    pub display_name: String,
    /// `moderator` ou `admin`.
    pub role: String,
    /// Statut effectif : `banned` (compte), `pending` (compte ou appareil pas encore
    /// validé) ou `active`.
    pub status: String,
    pub device_id: i64,
    /// Code de l'appareil, à comparer par l'admin avant validation.
    pub fingerprint: String,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

#[derive(Default)]
pub struct TokenCache {
    entries: DashMap<String, (AuthUser, Instant)>,
}

#[derive(sqlx::FromRow)]
struct TokenRow {
    id: i64,
    last_used_at: i64,
    fingerprint: String,
    vrc_id: String,
    display_name: String,
    role: String,
    status: String,
}

/// Statut effectif d'un appareil `d` de l'utilisateur `u`.
pub const EFFECTIVE_STATUS: &str = "CASE WHEN u.status = 'banned' THEN 'banned'
    WHEN u.status = 'pending' OR d.status = 'pending' THEN 'pending' ELSE 'active' END";

impl TokenCache {
    /// Compte associé au jeton, s'il est valide (appareil non révoqué, jeton non expiré).
    pub async fn resolve(&self, pool: &PgPool, token: &str, ttl_days: i64) -> ApiResult<Option<AuthUser>> {
        if !token.starts_with(TOKEN_PREFIX) || token.len() > 128 {
            return Ok(None);
        }
        let hash = token_hash(token);
        if let Some(entry) = self.entries.get(&hash) {
            if entry.1.elapsed() < CACHE_TTL {
                return Ok(Some(entry.0.clone()));
            }
        }
        let now = now_unix();
        let row: Option<TokenRow> = sqlx::query_as(&format!(
            "SELECT d.id, d.last_used_at, d.fingerprint, u.vrc_id, u.display_name, u.role,
                    {EFFECTIVE_STATUS} AS status
             FROM devices d JOIN users u ON u.vrc_id = d.user_id
             WHERE d.token_hash = $1 AND d.status <> 'revoked' AND d.token_expires_at > $2"
        ))
        .bind(&hash)
        .bind(now)
        .fetch_optional(pool)
        .await?;
        let Some(row) = row else {
            self.entries.remove(&hash);
            return Ok(None);
        };
        if now - row.last_used_at > TOUCH_EVERY_SECS {
            sqlx::query("UPDATE devices SET last_used_at = $2, token_expires_at = $3 WHERE id = $1")
                .bind(row.id)
                .bind(now)
                .bind(now + ttl_days * 86_400)
                .execute(pool)
                .await?;
            sqlx::query("UPDATE users SET last_seen_at = $2 WHERE vrc_id = $1")
                .bind(&row.vrc_id)
                .bind(now)
                .execute(pool)
                .await?;
        }
        let user = AuthUser {
            id: row.vrc_id,
            display_name: row.display_name,
            role: row.role,
            status: row.status,
            device_id: row.id,
            fingerprint: row.fingerprint,
        };
        self.entries.insert(hash, (user.clone(), Instant::now()));
        Ok(Some(user))
    }

    /// Après un changement de statut, de rôle ou une révocation : effet immédiat.
    pub fn forget_user(&self, user_id: &str) {
        self.entries.retain(|_, (user, _)| user.id != user_id);
    }

    pub fn sweep(&self) {
        self.entries.retain(|_, (_, at)| at.elapsed() < CACHE_TTL);
    }
}

/// Nouveau jeton de session pour l'appareil (l'ancien cesse de fonctionner). Le jeton en
/// clair n'est renvoyé qu'une fois.
pub async fn issue(tx: &mut Transaction<'_, Postgres>, device_id: i64, ttl_days: i64) -> ApiResult<String> {
    let token = random_token(TOKEN_PREFIX);
    let now = now_unix();
    sqlx::query("UPDATE devices SET token_hash = $2, token_expires_at = $3, last_used_at = $4 WHERE id = $1")
        .bind(device_id)
        .bind(token_hash(&token))
        .bind(now + ttl_days * 86_400)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    Ok(token)
}
