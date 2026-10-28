//! Configuration lue dans l'environnement (fichier `.env` de Docker Compose).
//!
//! Chaque secret peut aussi être fourni par fichier (`NOM_FILE=/run/secrets/…`), comme
//! le recommandent les secrets Docker.

use crate::proxy::{base_path, Cidr, ProxyConfig, PRIVATE_NETWORKS};
use anyhow::{bail, Context, Result};
use std::net::SocketAddr;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind: SocketAddr,
    /// Adresse publique (`https://sentinel.exemple.fr` ou `https://exemple.fr/sentinel`),
    /// telle que les applis et les navigateurs la voient, derrière le proxy éventuel.
    pub public_url: String,
    /// Chemin de `public_url` (`""` à la racine, sinon `/sentinel`).
    pub base_path: String,
    /// Durée de vie d'un jeton de session sans utilisation (l'appli en redemande un seule).
    pub token_ttl_days: i64,
    /// Proxys de confiance et en-tête qui donne l'adresse réelle du client.
    pub proxy: ProxyConfig,
    pub db_max_connections: u32,
}

fn var(name: &str) -> Result<Option<String>> {
    if let Ok(path) = std::env::var(format!("{name}_FILE")) {
        let value = std::fs::read_to_string(&path).with_context(|| format!("lecture de {name}_FILE ({path})"))?;
        return Ok(Some(value.trim().to_string()));
    }
    Ok(std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()))
}

/// `TRUSTED_PROXIES` : IP ou plages CIDR séparées par des virgules (`private` = réseaux
/// privés et la machine elle-même). `TRUST_PROXY=true` seul revient à `private`.
/// `REAL_IP_HEADER` : en-tête qui porte l'adresse du client (`x-forwarded-for` par défaut).
fn proxy_config() -> Result<ProxyConfig> {
    let trust = var("TRUST_PROXY")?.is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let list = match var("TRUSTED_PROXIES")? {
        Some(list) => list,
        None if trust => "private".into(),
        None => String::new(),
    };
    let mut trusted = Vec::new();
    for item in list.split([',', ' ', ';']).map(str::trim).filter(|i| !i.is_empty()) {
        if item.eq_ignore_ascii_case("private") {
            trusted.extend(PRIVATE_NETWORKS.iter().map(|n| Cidr::parse(n).expect("réseau privé valide")));
        } else {
            trusted.push(Cidr::parse(item).context("TRUSTED_PROXIES")?);
        }
    }
    let header = var("REAL_IP_HEADER")?.unwrap_or_else(|| "x-forwarded-for".into()).to_ascii_lowercase();
    if axum::http::HeaderName::from_bytes(header.as_bytes()).is_err() {
        bail!("REAL_IP_HEADER : nom d'en-tête invalide « {header} »");
    }
    Ok(ProxyConfig { trusted, header })
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let database_url = var("DATABASE_URL")?.context("DATABASE_URL manquant")?;
        let public_url = var("PUBLIC_URL")?
            .unwrap_or_else(|| "http://localhost:8080".into())
            .trim_end_matches('/')
            .to_string();
        if !(public_url.starts_with("https://") || public_url.starts_with("http://localhost") || public_url.starts_with("http://127.0.0.1")) {
            bail!("PUBLIC_URL doit commencer par https:// (ou http://localhost pour un essai)");
        }
        let base_path = base_path(&public_url)?;
        Ok(Self {
            database_url,
            bind: var("BIND")?
                .unwrap_or_else(|| "0.0.0.0:8080".into())
                .parse()
                .context("BIND invalide")?,
            public_url,
            base_path,
            token_ttl_days: var("TOKEN_TTL_DAYS")?.map_or(Ok(30), |v| v.parse()).context("TOKEN_TTL_DAYS")?,
            proxy: proxy_config()?,
            db_max_connections: var("DB_MAX_CONNECTIONS")?
                .map_or(Ok(16), |v| v.parse())
                .context("DB_MAX_CONNECTIONS")?,
        })
    }
}
