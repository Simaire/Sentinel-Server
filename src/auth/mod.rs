//! Identification des applis : jeton de session (`Authorization: Bearer sst_…`), obtenu
//! en signant un défi avec la clé de l'appareil (voir `device`).

pub mod device;
pub mod tokens;

use crate::error::ApiError;
use crate::state::Shared;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use std::net::{IpAddr, SocketAddr};
use std::ops::Deref;
pub use tokens::AuthUser;

fn bearer(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

async fn authenticate(parts: &Parts, state: &Shared) -> Result<AuthUser, ApiError> {
    let token = bearer(parts).ok_or(ApiError::Unauthorized)?;
    let user = state
        .tokens
        .resolve(&state.pool, token, state.cfg.token_ttl_days)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    if user.status == "banned" {
        return Err(ApiError::Forbidden("Compte banni de ce serveur.".into()));
    }
    Ok(user)
}

/// Compte identifié, même en attente de validation.
pub struct AnyUser(pub AuthUser);

impl FromRequestParts<Shared> for AnyUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Shared) -> Result<Self, Self::Rejection> {
        authenticate(parts, state).await.map(AnyUser)
    }
}

/// Compte et appareil validés par un admin.
pub struct ActiveUser(pub AuthUser);

impl Deref for ActiveUser {
    type Target = AuthUser;
    fn deref(&self) -> &AuthUser {
        &self.0
    }
}

impl FromRequestParts<Shared> for ActiveUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Shared) -> Result<Self, Self::Rejection> {
        let user = authenticate(parts, state).await?;
        match user.status.as_str() {
            "active" => Ok(ActiveUser(user)),
            _ => Err(ApiError::Pending),
        }
    }
}

/// Adresse réelle du client, derrière les proxys de confiance (voir `crate::proxy`).
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<Shared> for ClientIp {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Shared) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
            .unwrap_or(IpAddr::from([0, 0, 0, 0]));
        Ok(ClientIp(state.cfg.proxy.client_ip(peer, &parts.headers)))
    }
}
