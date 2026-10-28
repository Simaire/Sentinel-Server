//! Fichiers de l'interface d'administration, intégrés au binaire à la compilation
//! (`web/dist`). Les fichiers versionnés (`assets/…`) sont mis en cache un an.

use axum::extract::Path;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

/// Politique de sécurité du contenu de l'interface : rien d'extérieur, pas de cadre.
pub const ADMIN_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
    img-src 'self' data:; connect-src 'self'; font-src 'self'; frame-ancestors 'none'; base-uri 'none'; \
    form-action 'self'";

fn serve(path: &str) -> Response {
    let (file, name) = match Assets::get(path) {
        Some(file) => (file, path),
        // Application à page unique : toute autre adresse affiche index.html.
        None => match Assets::get("index.html") {
            Some(file) => (file, "index.html"),
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Interface d'administration absente : construire web/ (npm run build).",
                )
                    .into_response()
            }
        },
    };
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut response = (StatusCode::OK, file.data.into_owned()).into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        headers.insert(CONTENT_TYPE, value);
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

pub async fn index() -> Response {
    serve("index.html")
}

pub async fn file(Path(path): Path<String>) -> Response {
    serve(path.trim_start_matches('/'))
}
