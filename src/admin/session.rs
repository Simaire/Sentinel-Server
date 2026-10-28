//! Connexion à l'interface web d'administration, sans mot de passe :
//!
//! - depuis l'appli Sentinel d'un admin : `POST /api/v1/admin/link` donne un lien à usage
//!   unique (2 min) qui ouvre une session dans le navigateur ;
//! - ou sur le serveur : `docker compose exec server sentinel-server admin-link usr_…`.
//!
//! La session est un cookie `HttpOnly`, `SameSite=Strict` (et `Secure` en HTTPS) ; les
//! requêtes qui modifient quelque chose exigent en plus l'en-tête `X-Sentinel: 1`.

use crate::auth::{ActiveUser, ClientIp};
use crate::error::{ApiError, ApiResult};
use crate::state::Shared;
use crate::util::{now_unix, random_token, token_hash};
use anyhow::{bail, Result};
use axum::extract::{FromRequestParts, Query, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

pub const COOKIE_NAME: &str = "sentinel_admin";
const LINK_TTL_SECS: i64 = 120;
const SESSION_TTL_SECS: i64 = 12 * 3_600;

/// Crée un lien de connexion à usage unique pour cet admin.
pub async fn create_link(pool: &PgPool, public_url: &str, user_id: &str) -> Result<String> {
    let allowed: Option<bool> =
        sqlx::query_scalar("SELECT role = 'admin' AND status = 'active' FROM users WHERE vrc_id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    if allowed != Some(true) {
        bail!("{user_id} n'est pas un administrateur actif (il doit s'être connecté une fois depuis l'appli)");
    }
    let code = random_token("");
    sqlx::query("DELETE FROM admin_codes WHERE expires_at < $1")
        .bind(now_unix())
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO admin_codes (code_hash, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(token_hash(&code))
        .bind(user_id)
        .bind(now_unix() + LINK_TTL_SECS)
        .execute(pool)
        .await?;
    Ok(format!("{public_url}/admin/login?code={code}"))
}

#[derive(Serialize)]
pub struct LinkResponse {
    pub url: String,
    pub expires_in: i64,
}

pub async fn link(State(state): State<Shared>, user: ActiveUser) -> ApiResult<Json<LinkResponse>> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("Réservé aux administrateurs.".into()));
    }
    let url = create_link(&state.pool, &state.cfg.public_url, &user.id).await?;
    Ok(Json(LinkResponse {
        url,
        expires_in: LINK_TTL_SECS,
    }))
}

#[derive(Deserialize)]
pub struct LoginQuery {
    pub code: String,
}

/// Cookie de session : invisible du JavaScript, jamais envoyé depuis un autre site, et
/// limité au chemin du serveur (`base` : `""` ou `/sentinel`) s'il partage son domaine.
fn session_cookie(value: &str, max_age: i64, secure: bool, base: &str) -> String {
    format!(
        "{COOKIE_NAME}={value}; Path={base}/; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
}

/// Valeur du cookie de session dans l'en-tête `Cookie`.
fn read_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == COOKIE_NAME && !value.is_empty()).then(|| value.to_string())
        })
}

pub async fn login(
    State(state): State<Shared>,
    ClientIp(ip): ClientIp,
    Query(query): Query<LoginQuery>,
) -> Result<Response, ApiError> {
    if !state.limits.admin_login.allow(&ip.to_string()) {
        return Err(ApiError::TooManyRequests);
    }
    let now = now_unix();
    let user: Option<String> = sqlx::query_scalar(
        "DELETE FROM admin_codes WHERE code_hash = $1 AND expires_at > $2 RETURNING user_id",
    )
    .bind(token_hash(&query.code))
    .bind(now)
    .fetch_optional(&state.pool)
    .await?;
    let Some(user_id) = user else {
        return Ok(Redirect::to(&format!("{}/admin/?login=expired", state.cfg.base_path)).into_response());
    };
    let session = random_token("ses_");
    sqlx::query("DELETE FROM admin_sessions WHERE expires_at < $1")
        .bind(now)
        .execute(&state.pool)
        .await?;
    sqlx::query("INSERT INTO admin_sessions (id_hash, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)")
        .bind(token_hash(&session))
        .bind(&user_id)
        .bind(now)
        .bind(now + SESSION_TTL_SECS)
        .execute(&state.pool)
        .await?;
    crate::db::audit_now(&state.pool, Some(&user_id), "admin.login", None, Some(&ip.to_string())).await;
    let secure = state.cfg.public_url.starts_with("https://");
    let cookie = session_cookie(&session, SESSION_TTL_SECS, secure, &state.cfg.base_path);
    Ok(([(SET_COOKIE, cookie)], Redirect::to(&format!("{}/admin/", state.cfg.base_path))).into_response())
}

/// Admin connecté à l'interface web.
pub struct AdminSession {
    pub user_id: String,
    pub display_name: String,
    session_hash: String,
}

impl FromRequestParts<Shared> for AdminSession {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Shared) -> Result<Self, Self::Rejection> {
        // Protection CSRF : un autre site ne peut pas ajouter cet en-tête sans autorisation CORS.
        if parts.method != Method::GET && parts.headers.get("x-sentinel").and_then(|v| v.to_str().ok()) != Some("1") {
            return Err(ApiError::Forbidden("En-tête X-Sentinel manquant.".into()));
        }
        let session = read_cookie(&parts.headers).ok_or(ApiError::Unauthorized)?;
        let hash = token_hash(&session);
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT u.vrc_id, u.display_name FROM admin_sessions s JOIN users u ON u.vrc_id = s.user_id
             WHERE s.id_hash = $1 AND s.expires_at > $2 AND u.role = 'admin' AND u.status = 'active'",
        )
        .bind(&hash)
        .bind(now_unix())
        .fetch_optional(&state.pool)
        .await?;
        let (user_id, display_name) = row.ok_or(ApiError::Unauthorized)?;
        Ok(Self {
            user_id,
            display_name,
            session_hash: hash,
        })
    }
}

#[derive(Serialize)]
pub struct AdminMe {
    pub user_id: String,
    pub display_name: String,
}

pub async fn me(admin: AdminSession) -> Json<AdminMe> {
    Json(AdminMe {
        user_id: admin.user_id,
        display_name: admin.display_name,
    })
}

pub async fn logout(State(state): State<Shared>, admin: AdminSession) -> ApiResult<Response> {
    sqlx::query("DELETE FROM admin_sessions WHERE id_hash = $1")
        .bind(&admin.session_hash)
        .execute(&state.pool)
        .await?;
    let removal = session_cookie("", 0, state.cfg.public_url.starts_with("https://"), &state.cfg.base_path);
    Ok(([(SET_COOKIE, removal)], Json(serde_json::json!({ "ok": true }))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cookie_is_locked_down() {
        let cookie = session_cookie("abc", 60, true, "");
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict") && cookie.ends_with("Secure"));
        assert!(cookie.contains("Path=/;"));
        assert!(session_cookie("abc", 60, true, "/sentinel").contains("Path=/sentinel/;"));
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, "theme=dark; sentinel_admin=ses_xyz; other=1".parse().unwrap());
        assert_eq!(read_cookie(&headers).as_deref(), Some("ses_xyz"));
    }
}
