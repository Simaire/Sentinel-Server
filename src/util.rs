//! Petits utilitaires communs.

use rand::RngCore;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// `usr_` suivi d'un UUID (ou d'un identifiant ancien, alphanumérique).
pub fn is_user_id(id: &str) -> bool {
    id.len() <= 64
        && id
            .strip_prefix("usr_")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// Jeton aléatoire de 256 bits, encodé en base64url, avec un préfixe lisible.
pub fn random_token(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!(
        "{prefix}{}",
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
    )
}

/// Empreinte SHA-256 (hex) d'un jeton : seule l'empreinte est stockée en base.
pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Code d'appareil affiché dans l'appli et dans l'administration (`ABCD-EF12-3456-7890`) :
/// les 64 premiers bits de l'empreinte SHA-256 de la clé publique. L'admin le compare
/// avec celui que lui donne le modo avant de valider l'appareil.
pub fn fingerprint(public_key: &[u8]) -> String {
    let digest = Sha256::digest(public_key);
    digest[..8]
        .chunks(2)
        .map(hex::encode_upper)
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_tokens_and_codes() {
        assert!(is_user_id("usr_c1644b5b-3ca4-45b4-97c6-a2a0de70d469"));
        assert!(!is_user_id("usr_"));
        assert!(!is_user_id("grp_x"));
        assert!(!is_user_id("usr_a'; DROP TABLE users;--"));
        let token = random_token("sst_");
        assert!(token.starts_with("sst_") && token.len() > 40);
        assert_ne!(random_token("x"), random_token("x"));
        assert_eq!(token_hash("abc").len(), 64);
        // Même valeur que dans l'appli (`server::fingerprint`).
        assert_eq!(fingerprint(&[0u8; 32]), "6668-7AAD-F862-BD77");
    }
}
