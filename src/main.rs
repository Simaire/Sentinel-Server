//! Serveur VRC Sentinel : synchronisation des applis des modérateurs et administration.

mod admin;
mod auth;
mod config;
mod db;
mod error;
mod presence;
mod proxy;
mod routes;
mod state;
mod sync;
mod util;
mod web;

#[cfg(test)]
mod tests_pg;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

#[derive(Parser)]
#[command(name = "sentinel-server", version, about = "Serveur de synchronisation de VRC Sentinel")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Lance le serveur (par défaut).
    Serve,
    /// Valide un appareil d'après le code affiché dans l'appli (premier admin, dépannage).
    Approve {
        /// Code de l'appareil, par exemple ABCD-EF12-3456-7890.
        fingerprint: String,
        /// Donne aussi le rôle d'administrateur au compte.
        #[arg(long)]
        admin: bool,
    },
    /// Affiche un lien de connexion à l'administration, valable 2 minutes.
    AdminLink {
        /// Compte VRChat de l'admin (usr_…).
        user_id: String,
    },
    /// Vérifie que le serveur répond (utilisé par Docker).
    Healthcheck {
        #[arg(default_value = "http://127.0.0.1:8080/health")]
        url: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sentinel_server=info,tower_http=warn".into()),
        )
        .with_target(false)
        .init();
    match Cli::parse().command.unwrap_or(Command::Serve) {
        Command::Serve => serve().await,
        Command::Approve { fingerprint, admin } => {
            let cfg = config::Config::from_env()?;
            let pool = db::connect(&cfg.database_url, 1).await?;
            let user = approve(&pool, &fingerprint, admin).await?;
            println!("Appareil {fingerprint} validé pour {user}.");
            if admin {
                println!("{}", admin::session::create_link(&pool, &cfg.public_url, &user).await?);
            }
            Ok(())
        }
        Command::AdminLink { user_id } => {
            let cfg = config::Config::from_env()?;
            let pool = db::connect(&cfg.database_url, 1).await?;
            println!("{}", admin::session::create_link(&pool, &cfg.public_url, &user_id).await?);
            Ok(())
        }
        Command::Healthcheck { url } => {
            let ok = tokio::time::timeout(Duration::from_secs(3), healthcheck(&url)).await.is_ok_and(|ok| ok);
            std::process::exit(if ok { 0 } else { 1 });
        }
    }
}

/// Valide l'appareil (et son compte) depuis la ligne de commande du serveur : c'est ainsi
/// que le premier admin est créé, sans rien de spécial dans la configuration.
async fn approve(pool: &sqlx::PgPool, fingerprint: &str, admin: bool) -> Result<String> {
    let code = fingerprint.trim().to_ascii_uppercase();
    let mut tx = pool.begin().await?;
    let users: Vec<String> = sqlx::query_scalar(
        "UPDATE devices SET status = 'active', approved_at = $2, approved_by = 'console'
         WHERE fingerprint = $1 AND status <> 'revoked' RETURNING user_id",
    )
    .bind(&code)
    .bind(util::now_unix())
    .fetch_all(&mut *tx)
    .await?;
    let [user] = users.as_slice() else {
        anyhow::bail!("aucun appareil (ou plusieurs) avec le code {code} : connecte d'abord l'appli au serveur");
    };
    sqlx::query(
        "UPDATE users SET status = 'active', approved_at = $2, approved_by = 'console',
            role = CASE WHEN $3 THEN 'admin' ELSE role END
         WHERE vrc_id = $1",
    )
    .bind(user)
    .bind(util::now_unix())
    .bind(admin)
    .execute(&mut *tx)
    .await?;
    db::audit(&mut tx, Some("console"), if admin { "device.approve.admin" } else { "device.approve" }, Some(user), Some(&code)).await?;
    tx.commit().await?;
    Ok(user.clone())
}

/// `GET /health` minimal, sans client HTTP complet (l'image n'a ni shell ni curl).
async fn healthcheck(url: &str) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let (host, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(h, p)| (h, format!("/{p}")));
    let Ok(mut stream) = tokio::net::TcpStream::connect(host).await else {
        return false;
    };
    let request = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response).await;
    response.starts_with(b"HTTP/1.0 200") || response.starts_with(b"HTTP/1.1 200")
}

async fn serve() -> Result<()> {
    let cfg = config::Config::from_env()?;
    let pool = db::connect(&cfg.database_url, cfg.db_max_connections).await?;
    let (head_tx, head_rx) = watch::channel(db::current_head(&pool).await?);
    let writer = sync::writer::Writer::spawn(pool.clone(), head_tx);
    let bind = cfg.bind;
    let state_url = cfg.public_url.clone();
    let (proxies, header) = (cfg.proxy.trusted.len(), cfg.proxy.header.clone());
    let state = Arc::new(state::AppState {
        cfg,
        pool,
        writer,
        head: head_rx,
        tokens: Default::default(),
        challenges: Default::default(),
        presence: Default::default(),
        limits: Default::default(),
        metrics: Default::default(),
        started: Instant::now(),
    });

    // Ménage périodique des caches et limiteurs en mémoire.
    let housekeeping = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            housekeeping.limits.sweep();
            housekeeping.tokens.sweep();
            housekeeping.challenges.sweep();
            housekeeping.presence.sweep();
        }
    });

    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("écoute sur {bind}"))?;
    tracing::info!("VRC Sentinel Server {} à l'écoute sur {bind}", env!("CARGO_PKG_VERSION"));
    tracing::info!("adresse publique : {}", state_url);
    if proxies == 0 {
        tracing::info!("aucun proxy de confiance : l'adresse de connexion fait foi");
    } else {
        tracing::info!("{proxies} réseau(x) de proxy de confiance, adresse du client lue dans {header}");
    }
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(shutdown())
        .await?;
    tracing::info!("arrêt propre");
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
