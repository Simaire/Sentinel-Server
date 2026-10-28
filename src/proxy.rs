//! Fonctionnement derrière un reverse proxy (Caddy, nginx, Traefik, Nginx Proxy Manager,
//! Cloudflare…).
//!
//! - **Adresse du client** : les limites de débit et le journal d'audit utilisent l'IP réelle.
//!   Elle n'est lue dans les en-têtes que si la connexion vient d'un proxy de confiance
//!   (`TRUSTED_PROXIES`). Dans `X-Forwarded-For`, on remonte depuis la droite et on prend la
//!   première adresse qui n'est pas un proxy de confiance : celles écrites plus à gauche
//!   viennent du client et peuvent être inventées.
//! - **Sous-chemin** : avec `PUBLIC_URL=https://exemple.fr/sentinel`, le serveur répond sous
//!   `/sentinel/…`, que le proxy retire ce préfixe ou non (voir `routes`).

use anyhow::{bail, Context, Result};
use axum::http::HeaderMap;
use std::net::IpAddr;

/// Réseau autorisé à transmettre l'adresse du client : une IP ou une plage CIDR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let (addr, prefix) = match text.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (text, None),
        };
        let addr: IpAddr = addr.parse().with_context(|| format!("adresse invalide : {text}"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.parse::<u8>().with_context(|| format!("préfixe invalide : {text}"))?,
            None => max,
        };
        if prefix > max {
            bail!("préfixe trop long : {text}");
        }
        Ok(Self { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        // Une IPv4 vue en IPv6 (::ffff:a.b.c.d) est comparée comme une IPv4.
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            v4 => v4,
        };
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.prefix)).unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(self.prefix)).unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// Réseaux de confiance par défaut avec `TRUST_PROXY=true` : la machine elle-même et les
/// réseaux privés (Docker, réseau local). Un proxy public (Cloudflare…) se déclare dans
/// `TRUSTED_PROXIES`.
pub const PRIVATE_NETWORKS: &[&str] = &[
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "100.64.0.0/10",
    "::1/128",
    "fc00::/7",
    "fe80::/10",
];

/// Comment retrouver l'adresse du client derrière les proxys.
#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// Vide : aucun proxy de confiance, l'adresse de la connexion fait foi.
    pub trusted: Vec<Cidr>,
    /// En-tête lu (en minuscules) : `x-forwarded-for`, `forwarded`, `x-real-ip`,
    /// `cf-connecting-ip`…
    pub header: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            trusted: Vec::new(),
            header: "x-forwarded-for".into(),
        }
    }
}

impl ProxyConfig {
    pub fn is_trusted(&self, ip: IpAddr) -> bool {
        self.trusted.iter().any(|net| net.contains(ip))
    }

    /// Adresse du client : celle de la connexion, sauf si elle vient d'un proxy de confiance
    /// qui transmet l'adresse d'origine.
    pub fn client_ip(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        if !self.is_trusted(peer) {
            return peer;
        }
        let values = headers
            .get_all(self.header.as_str())
            .iter()
            .filter_map(|v| v.to_str().ok());
        let chain: Vec<IpAddr> = match self.header.as_str() {
            "x-forwarded-for" => values.flat_map(|v| v.split(',')).filter_map(parse_ip).collect(),
            "forwarded" => values
                .flat_map(|v| v.split([',', ';']))
                .filter_map(|part| {
                    let (key, value) = part.trim().split_once('=')?;
                    key.trim().eq_ignore_ascii_case("for").then(|| parse_ip(value)).flatten()
                })
                .collect(),
            // En-tête à valeur unique, écrit par le proxy lui-même (X-Real-IP, CF-Connecting-IP…).
            _ => values.filter_map(parse_ip).next_back().into_iter().collect(),
        };
        // De droite à gauche : le dernier proxy a écrit l'adresse qui l'a contacté.
        chain
            .into_iter()
            .rev()
            .find(|ip| !self.is_trusted(*ip))
            .unwrap_or(peer)
    }
}

/// `1.2.3.4`, `1.2.3.4:5678`, `[2001:db8::1]:443`, `"[2001:db8::1]"`…
fn parse_ip(text: &str) -> Option<IpAddr> {
    let text = text.trim().trim_matches('"');
    if let Ok(ip) = text.parse() {
        return Some(ip);
    }
    if let Some(rest) = text.strip_prefix('[') {
        return rest.split(']').next()?.parse().ok();
    }
    text.rsplit_once(':')
        .filter(|(host, _)| !host.contains(':'))
        .and_then(|(host, _)| host.parse().ok())
}

/// Chemin sous lequel le serveur est publié, d'après `PUBLIC_URL` : `""` à la racine,
/// sinon `/chemin` (sans barre finale).
pub fn base_path(public_url: &str) -> Result<String> {
    let rest = public_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .context("PUBLIC_URL sans schéma")?;
    let path = rest.find('/').map_or("", |i| &rest[i..]).trim_end_matches('/');
    if path.contains(['?', '#', ' ', '\\']) || path.split('/').any(|seg| seg == ".." || seg == ".") {
        bail!("PUBLIC_URL : chemin invalide « {path} »");
    }
    if path == "/api" || path.starts_with("/api/") || path == "/admin" || path.starts_with("/admin/") {
        bail!("PUBLIC_URL : le chemin ne peut pas commencer par /api ou /admin");
    }
    Ok(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn headers(name: &'static str, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(name, value.parse().unwrap());
        headers
    }

    fn private() -> ProxyConfig {
        ProxyConfig {
            trusted: PRIVATE_NETWORKS.iter().map(|n| Cidr::parse(n).unwrap()).collect(),
            header: "x-forwarded-for".into(),
        }
    }

    #[test]
    fn cidr_ranges() {
        let net = Cidr::parse("172.16.0.0/12").unwrap();
        assert!(net.contains(ip("172.20.1.2")) && !net.contains(ip("172.32.0.1")));
        assert!(Cidr::parse("10.0.0.5").unwrap().contains(ip("10.0.0.5")));
        assert!(Cidr::parse("0.0.0.0/0").unwrap().contains(ip("8.8.8.8")));
        assert!(Cidr::parse("fc00::/7").unwrap().contains(ip("fd12::1")));
        assert!(Cidr::parse("127.0.0.0/8").unwrap().contains(ip("::ffff:127.0.0.1")), "IPv4 vue en IPv6");
        assert!(Cidr::parse("10.0.0.0/33").is_err() && Cidr::parse("pas-une-ip").is_err());
    }

    #[test]
    fn forwarded_for_is_read_from_the_right() {
        let cfg = private();
        // Le client a glissé une fausse adresse à gauche : elle est ignorée.
        let h = headers("x-forwarded-for", "6.6.6.6, 203.0.113.7, 172.18.0.3");
        assert_eq!(cfg.client_ip(ip("172.18.0.2"), &h), ip("203.0.113.7"));
        // Connexion directe (pas un proxy de confiance) : l'en-tête est ignoré.
        assert_eq!(cfg.client_ip(ip("198.51.100.9"), &h), ip("198.51.100.9"));
        // Sans en-tête : l'adresse du proxy.
        assert_eq!(cfg.client_ip(ip("172.18.0.2"), &HeaderMap::new()), ip("172.18.0.2"));
    }

    #[test]
    fn other_headers() {
        let mut cfg = private();
        cfg.header = "forwarded".into();
        let h = headers("forwarded", "for=192.0.2.60;proto=https, for=\"[2001:db8::7]:4711\"");
        assert_eq!(cfg.client_ip(ip("10.0.0.1"), &h), ip("2001:db8::7"));
        cfg.header = "cf-connecting-ip".into();
        let h = headers("cf-connecting-ip", "198.51.100.20");
        assert_eq!(cfg.client_ip(ip("10.0.0.1"), &h), ip("198.51.100.20"));
        assert_eq!(parse_ip("203.0.113.1:8080"), Some(ip("203.0.113.1")));
    }

    #[test]
    fn base_path_from_public_url() {
        assert_eq!(base_path("https://s.exemple.fr").unwrap(), "");
        assert_eq!(base_path("https://exemple.fr/sentinel/").unwrap(), "/sentinel");
        assert_eq!(base_path("https://exemple.fr/outils/sentinel").unwrap(), "/outils/sentinel");
        assert!(base_path("https://exemple.fr/admin").is_err());
        assert!(base_path("https://exemple.fr/a/../b").is_err());
    }
}
