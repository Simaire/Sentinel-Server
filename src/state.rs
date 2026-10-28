//! État partagé du serveur : base, écrivain, caches, limites et compteurs.

use crate::auth::device::Challenges;
use crate::auth::tokens::TokenCache;
use crate::config::Config;
use crate::presence::Presence;
use crate::sync::writer::Writer;
use dashmap::DashMap;
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

pub type Shared = Arc<AppState>;

pub struct AppState {
    pub cfg: Config,
    pub pool: PgPool,
    pub writer: Writer,
    /// Dernier numéro d'écriture, diffusé aux attentes longues.
    pub head: watch::Receiver<i64>,
    pub tokens: TokenCache,
    pub challenges: Challenges,
    pub presence: Presence,
    pub limits: Limits,
    pub metrics: Metrics,
    pub started: Instant,
}

/// Limiteur « seau de jetons » par clé (adresse IP, compte…).
pub struct RateLimiter {
    /// Jetons regagnés par seconde.
    rate: f64,
    burst: f64,
    buckets: DashMap<String, (f64, Instant)>,
}

impl RateLimiter {
    pub fn new(per_minute: f64, burst: f64) -> Self {
        Self {
            rate: per_minute / 60.0,
            burst,
            buckets: DashMap::new(),
        }
    }

    /// Consomme un jeton pour `key` ; faux si la limite est atteinte.
    pub fn allow(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut entry = self.buckets.entry(key.to_string()).or_insert((self.burst, now));
        let (tokens, last) = *entry;
        let refilled = (tokens + now.duration_since(last).as_secs_f64() * self.rate).min(self.burst);
        if refilled < 1.0 {
            *entry = (refilled, now);
            return false;
        }
        *entry = (refilled - 1.0, now);
        true
    }

    /// Oublie les clés pleines depuis longtemps (appelé périodiquement).
    pub fn sweep(&self) {
        let now = Instant::now();
        self.buckets.retain(|_, (tokens, last)| {
            *tokens + now.duration_since(*last).as_secs_f64() * self.rate < self.burst
        });
    }
}

pub struct Limits {
    /// Demandes de défi de connexion, par adresse IP.
    pub challenge_ip: RateLimiter,
    /// Connexions signées, par adresse IP.
    pub verify_ip: RateLimiter,
    /// Envois et relectures, par compte.
    pub sync: RateLimiter,
    /// Présence, par compte.
    pub presence: RateLimiter,
    /// Connexions à l'administration, par adresse IP.
    pub admin_login: RateLimiter,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            challenge_ip: RateLimiter::new(20.0, 10.0),
            verify_ip: RateLimiter::new(20.0, 10.0),
            sync: RateLimiter::new(240.0, 60.0),
            presence: RateLimiter::new(30.0, 10.0),
            admin_login: RateLimiter::new(10.0, 5.0),
        }
    }
}

impl Limits {
    pub fn sweep(&self) {
        for limiter in [
            &self.challenge_ip,
            &self.verify_ip,
            &self.sync,
            &self.presence,
            &self.admin_login,
        ] {
            limiter.sweep();
        }
    }
}

/// Compteurs depuis le démarrage, pour la page d'administration.
#[derive(Default)]
pub struct Metrics {
    pub pushes: AtomicU64,
    pub rows_written: AtomicU64,
    pub pulls: AtomicU64,
    pub rows_read: AtomicU64,
    /// Attentes longues en cours (≈ applis connectées en temps réel).
    pub waiting: AtomicU64,
    pub logins: AtomicU64,
    pub login_failures: AtomicU64,
}

impl Metrics {
    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_allows_a_burst_then_blocks() {
        let limiter = RateLimiter::new(60.0, 3.0);
        assert!(limiter.allow("a") && limiter.allow("a") && limiter.allow("a"));
        assert!(!limiter.allow("a"), "rafale épuisée");
        assert!(limiter.allow("b"), "clés indépendantes");
    }
}
