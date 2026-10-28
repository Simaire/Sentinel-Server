//! Connexion d'une appli par clé d'appareil, sans compte VRChat côté serveur et sans
//! aucun secret transmis :
//!
//! 1. À la première connexion, l'appli génère une paire de clés Ed25519. La clé privée
//!    reste dans le coffre-fort du système ; seule la clé publique est envoyée.
//! 2. `POST /api/v1/auth/challenge` : le serveur donne un défi à usage unique (2 min).
//! 3. `POST /api/v1/auth/login` : l'appli signe le défi, l'adresse du serveur, son compte
//!    VRChat et sa clé publique. Le serveur vérifie la signature et délivre un jeton.
//! 4. Un nouvel appareil reste « en attente » : un admin le valide après avoir comparé son
//!    code (empreinte de la clé) avec celui que le modo voit dans son appli.
//!
//! Le compte VRChat annoncé n'est donc pas prouvé par VRChat : c'est la validation de
//! l'appareil par un admin qui l'établit. Ensuite, seul ce PC peut agir pour ce compte.

use super::{AnyUser, ClientIp};
use crate::db::audit;
use crate::error::{ApiError, ApiResult};
use crate::state::Shared;
use crate::util::{fingerprint, is_user_id, now_unix, random_token};
use axum::extract::State;
use axum::Json;
use dashmap::DashMap;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const CHALLENGE_TTL: Duration = Duration::from_secs(120);
const MAX_CHALLENGES: usize = 10_000;
/// Appareils en attente par compte : au-delà, l'admin doit d'abord trier.
const MAX_PENDING_DEVICES: i64 = 5;
/// Préfixe du message signé (change si le format évolue).
pub const LOGIN_CONTEXT: &str = "sentinel-login-v1";

#[derive(Default)]
pub struct Challenges {
    pending: DashMap<String, Instant>,
}

impl Challenges {
    pub fn sweep(&self) {
        let now = Instant::now();
        self.pending.retain(|_, expires| *expires > now);
    }

    /// Consomme le défi : vrai s'il existait et n'avait pas expiré.
    fn take(&self, nonce: &str) -> bool {
        self.pending.remove(nonce).is_some_and(|(_, expires)| expires > Instant::now())
    }
}

/// Message signé par l'appli. L'adresse du serveur empêche de rejouer la signature
/// auprès d'un autre serveur.
pub fn login_message(server: &str, nonce: &str, user_id: &str, public_key: &str) -> String {
    format!("{LOGIN_CONTEXT}\n{server}\n{nonce}\n{user_id}\n{public_key}")
}

#[derive(Serialize)]
pub struct ChallengeResponse {
    pub nonce: String,
    /// Adresse du serveur à reprendre dans le message signé.
    pub server: String,
    pub expires_in: u64,
}

pub async fn challenge(State(state): State<Shared>, ClientIp(ip): ClientIp) -> ApiResult<Json<ChallengeResponse>> {
    if !state.limits.challenge_ip.allow(&ip.to_string()) {
        return Err(ApiError::TooManyRequests);
    }
    if state.challenges.pending.len() >= MAX_CHALLENGES {
        state.challenges.sweep();
        if state.challenges.pending.len() >= MAX_CHALLENGES {
            return Err(ApiError::TooManyRequests);
        }
    }
    let nonce = random_token("");
    state.challenges.pending.insert(nonce.clone(), Instant::now() + CHALLENGE_TTL);
    Ok(Json(ChallengeResponse {
        nonce,
        server: state.cfg.public_url.clone(),
        expires_in: CHALLENGE_TTL.as_secs(),
    }))
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub nonce: String,
    pub user_id: String,
    /// Pseudo VRChat, gardé pour un nouveau compte (l'admin le vérifie à la validation).
    #[serde(default)]
    pub display_name: String,
    /// Clé publique Ed25519, 32 octets en hexadécimal.
    pub public_key: String,
    /// Signature Ed25519 de [`login_message`], 64 octets en hexadécimal.
    pub signature: String,
    /// Nom de l'appareil, affiché dans l'administration.
    #[serde(default)]
    pub device: String,
}

#[derive(Serialize)]
pub struct UserView {
    pub id: String,
    pub display_name: String,
    pub status: String,
    pub role: String,
    /// Code de cet appareil.
    pub fingerprint: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: UserView,
}

fn clean(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

fn decode_key(public_key: &str, signature: &str) -> ApiResult<(VerifyingKey, Signature)> {
    let bad = || ApiError::BadRequest("Clé ou signature invalide.".into());
    let key: [u8; 32] = hex::decode(public_key).ok().and_then(|k| k.try_into().ok()).ok_or_else(bad)?;
    let sig: [u8; 64] = hex::decode(signature).ok().and_then(|s| s.try_into().ok()).ok_or_else(bad)?;
    Ok((VerifyingKey::from_bytes(&key).map_err(|_| bad())?, Signature::from_bytes(&sig)))
}

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: i64,
    user_id: String,
    status: String,
}

pub async fn login(
    State(state): State<Shared>,
    ClientIp(ip): ClientIp,
    Json(body): Json<LoginRequest>,
) -> ApiResult<Json<LoginResponse>> {
    if !state.limits.verify_ip.allow(&ip.to_string()) {
        return Err(ApiError::TooManyRequests);
    }
    if !is_user_id(&body.user_id) {
        return Err(ApiError::BadRequest("Identifiant VRChat invalide.".into()));
    }
    let public_key = body.public_key.to_ascii_lowercase();
    let (key, signature) = decode_key(&public_key, &body.signature)?;
    if !state.challenges.take(&body.nonce) {
        return Err(ApiError::BadRequest("Défi inconnu ou expiré : recommence la connexion.".into()));
    }
    let message = login_message(&state.cfg.public_url, &body.nonce, &body.user_id, &public_key);
    if key.verify_strict(message.as_bytes(), &signature).is_err() {
        state.metrics.login_failures.fetch_add(1, Ordering::Relaxed);
        crate::db::audit_now(&state.pool, Some(&body.user_id), "auth.bad_signature", None, Some(&ip.to_string())).await;
        return Err(ApiError::Forbidden("Signature refusée.".into()));
    }

    let user_id = body.user_id;
    let claimed_name = Some(clean(&body.display_name, 64)).filter(|n| !n.is_empty()).unwrap_or_else(|| user_id.clone());
    let device_name = clean(&body.device, 64);
    let code = fingerprint(key.as_bytes());
    let now = now_unix();

    let mut tx = state.pool.begin().await?;
    let user: Option<(String, String)> = sqlx::query_as("SELECT status, role FROM users WHERE vrc_id = $1 FOR UPDATE")
        .bind(&user_id)
        .fetch_optional(&mut *tx)
        .await?;
    if user.as_ref().is_some_and(|(status, _)| status == "banned") {
        return Err(ApiError::Forbidden("Compte banni de ce serveur.".into()));
    }
    let device: Option<DeviceRow> = sqlx::query_as("SELECT id, user_id, status FROM devices WHERE public_key = $1 FOR UPDATE")
        .bind(&public_key)
        .fetch_optional(&mut *tx)
        .await?;
    let (device_id, device_status) = match device {
        Some(d) if d.user_id != user_id => {
            return Err(ApiError::Forbidden("Cet appareil est lié à un autre compte VRChat.".into()));
        }
        Some(d) if d.status == "revoked" => {
            return Err(ApiError::Forbidden(
                "Cet appareil a été révoqué : déconnecte-toi puis reconnecte-toi pour en demander la validation.".into(),
            ));
        }
        Some(d) => {
            sqlx::query("UPDATE devices SET name = COALESCE(NULLIF($2, ''), name) WHERE id = $1")
                .bind(d.id)
                .bind(&device_name)
                .execute(&mut *tx)
                .await?;
            (d.id, d.status)
        }
        None => {
            if user.is_none() {
                let open = crate::db::get_state(&state.pool, "registration_open")
                    .await?
                    .is_none_or(|v| v != "false");
                if !open {
                    return Err(ApiError::Forbidden("Les inscriptions sont fermées sur ce serveur.".into()));
                }
                sqlx::query(
                    "INSERT INTO users (vrc_id, display_name, status, role, created_at, last_seen_at)
                     VALUES ($1, $2, 'pending', 'moderator', $3, $3)",
                )
                .bind(&user_id)
                .bind(&claimed_name)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM devices WHERE user_id = $1 AND status = 'pending'")
                .bind(&user_id)
                .fetch_one(&mut *tx)
                .await?;
            if waiting >= MAX_PENDING_DEVICES {
                return Err(ApiError::TooManyRequests);
            }
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO devices (user_id, public_key, fingerprint, name, status, created_at, last_used_at)
                 VALUES ($1, $2, $3, NULLIF($4, ''), 'pending', $5, $5) RETURNING id",
            )
            .bind(&user_id)
            .bind(&public_key)
            .bind(&code)
            .bind(&device_name)
            .bind(now)
            .fetch_one(&mut *tx)
            .await?;
            audit(&mut tx, Some(&user_id), "device.new", Some(&code), Some(&ip.to_string())).await?;
            (id, "pending".to_string())
        }
    };
    let (user_status, role) = user.unwrap_or_else(|| ("pending".into(), "moderator".into()));
    // Pseudo mis à jour seulement depuis un appareil déjà validé.
    if device_status == "active" && user_status == "active" {
        sqlx::query("UPDATE users SET display_name = $2 WHERE vrc_id = $1")
            .bind(&user_id)
            .bind(&claimed_name)
            .execute(&mut *tx)
            .await?;
    }
    let display_name: String = sqlx::query_scalar("SELECT display_name FROM users WHERE vrc_id = $1")
        .bind(&user_id)
        .fetch_one(&mut *tx)
        .await?;
    let token = super::tokens::issue(&mut tx, device_id, state.cfg.token_ttl_days).await?;
    tx.commit().await?;
    state.tokens.forget_user(&user_id);
    state.metrics.logins.fetch_add(1, Ordering::Relaxed);

    let status = if user_status == "active" && device_status == "active" {
        "active"
    } else {
        "pending"
    };
    Ok(Json(LoginResponse {
        token,
        user: UserView {
            id: user_id,
            display_name,
            status: status.into(),
            role,
            fingerprint: code,
        },
    }))
}

#[derive(Serialize)]
pub struct MeResponse {
    pub user: UserView,
    pub server_version: &'static str,
}

pub async fn me(AnyUser(user): AnyUser) -> Json<MeResponse> {
    Json(MeResponse {
        user: UserView {
            id: user.id,
            display_name: user.display_name,
            status: user.status,
            role: user.role,
            fingerprint: user.fingerprint,
        },
        server_version: env!("CARGO_PKG_VERSION"),
    })
}

/// Déconnexion de cet appareil : il est révoqué (une nouvelle connexion repassera par
/// la validation d'un admin).
pub async fn logout(State(state): State<Shared>, AnyUser(user): AnyUser) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("UPDATE devices SET status = 'revoked', token_hash = NULL WHERE id = $1")
        .bind(user.device_id)
        .execute(&state.pool)
        .await?;
    state.tokens.forget_user(&user.id);
    state.presence.remove(&user.id);
    crate::db::audit_now(&state.pool, Some(&user.id), "auth.logout", Some(&user.fingerprint), None).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn only_the_exact_message_verifies() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let public = hex::encode(signing.verifying_key().as_bytes());
        let message = login_message("https://s.exemple.fr", "n1", "usr_a", &public);
        let sig = hex::encode(signing.sign(message.as_bytes()).to_bytes());
        let (key, signature) = decode_key(&public, &sig).unwrap();
        assert!(key.verify_strict(message.as_bytes(), &signature).is_ok());
        for other in [
            login_message("https://autre.fr", "n1", "usr_a", &public),
            login_message("https://s.exemple.fr", "n2", "usr_a", &public),
            login_message("https://s.exemple.fr", "n1", "usr_b", &public),
        ] {
            assert!(key.verify_strict(other.as_bytes(), &signature).is_err(), "{other}");
        }
        assert!(decode_key("zz", &sig).is_err());
    }

    #[test]
    fn a_challenge_is_used_once() {
        let challenges = Challenges::default();
        challenges.pending.insert("n".into(), Instant::now() + CHALLENGE_TTL);
        assert!(challenges.take("n"));
        assert!(!challenges.take("n"), "pas de rejeu");
        challenges.pending.insert("old".into(), Instant::now() - Duration::from_secs(1));
        assert!(!challenges.take("old"), "expiré");
    }
}
