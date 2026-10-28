//! Assemblage des routes et des protections communes (taille des requêtes, délais,
//! compression, en-têtes de sécurité).

use crate::admin::{api as admin_api, session};
use crate::state::Shared;
use crate::{auth, presence, sync, web};
use axum::http::header::{
    CONTENT_SECURITY_POLICY, REFERRER_POLICY, STRICT_TRANSPORT_SECURITY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{HeaderValue, StatusCode};
use axum::extract::State;
use axum::response::Redirect;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::decompression::RequestDecompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Corps de requête maximal une fois décompressé.
const MAX_BODY: usize = 16 * 1024 * 1024;

async fn health(State(state): State<Shared>) -> Json<serde_json::Value> {
    let db = sqlx::query("SELECT 1").execute(&state.pool).await.is_ok();
    Json(json!({ "ok": db, "version": env!("CARGO_PKG_VERSION") }))
}

/// Vers l'interface d'administration, sous le chemin public (`PUBLIC_URL`).
async fn to_admin(State(state): State<Shared>) -> Redirect {
    Redirect::to(&format!("{}/admin/", state.cfg.base_path))
}

pub fn router(state: Shared) -> Router {
    let api = Router::new()
        .route("/auth/challenge", post(auth::device::challenge))
        .route("/auth/login", post(auth::device::login))
        .route("/auth/me", get(auth::device::me))
        .route("/auth/logout", post(auth::device::logout))
        .route("/sync/push", post(sync::push))
        .route("/sync/pull", post(sync::pull))
        .route("/sync/wait", get(sync::wait))
        .route("/presence", get(presence::list).post(presence::post))
        .route("/admin/link", post(session::link))
        .route("/admin/me", get(session::me))
        .route("/admin/logout", post(session::logout))
        .route("/admin/overview", get(admin_api::overview))
        .route("/admin/users", get(admin_api::users))
        .route("/admin/users/{id}", get(admin_api::user_detail).delete(admin_api::delete_user))
        .route("/admin/users/{id}/status", post(admin_api::set_status))
        .route("/admin/users/{id}/role", post(admin_api::set_role))
        .route("/admin/users/{id}/note", post(admin_api::set_note))
        .route("/admin/users/{id}/revoke-devices", post(admin_api::revoke_user_devices))
        .route("/admin/devices/{id}/approve", post(admin_api::approve_device))
        .route("/admin/devices/{id}", delete(admin_api::revoke_device))
        .route("/admin/tables", get(admin_api::tables))
        .route("/admin/tables/{name}", get(admin_api::table_rows))
        .route("/admin/reports/{id}", delete(admin_api::delete_report))
        .route("/admin/audit", get(admin_api::audit_log))
        .route("/admin/settings", get(admin_api::get_settings).post(admin_api::set_settings));

    let secure = state.cfg.public_url.starts_with("https://");
    let routes = Router::new()
        .route("/", get(to_admin))
        .route("/health", get(health))
        .route("/admin/login", get(session::login))
        .route("/admin", get(to_admin))
        .route("/admin/", get(web::index))
        .route("/admin/{*path}", get(web::file))
        .nest("/api/v1", api);
    // Publié sous un sous-chemin : servi avec et sans le préfixe, que le proxy le retire
    // (Traefik StripPrefix, nginx `proxy_pass …/`) ou le transmette tel quel.
    let base = state.cfg.base_path.clone();
    let routes = if base.is_empty() {
        routes
    } else {
        Router::new()
            .route(&format!("{base}/"), get(to_admin))
            .nest(&base, routes.clone())
            .merge(routes)
    };
    let mut app = routes
        .layer(RequestBodyLimitLayer::new(MAX_BODY))
        .layer(RequestDecompressionLayer::new())
        .layer(CompressionLayer::new())
        // Les attentes longues durent 25 s au plus.
        .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, Duration::from_secs(40)))
        .layer(SetResponseHeaderLayer::overriding(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::overriding(X_FRAME_OPTIONS, HeaderValue::from_static("DENY")))
        .layer(SetResponseHeaderLayer::overriding(REFERRER_POLICY, HeaderValue::from_static("no-referrer")))
        .layer(SetResponseHeaderLayer::if_not_present(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(web::ADMIN_CSP),
        ))
        .layer(TraceLayer::new_for_http());
    if secure {
        app = app.layer(SetResponseHeaderLayer::overriding(
            STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        ));
    }
    app.with_state(state)
}
