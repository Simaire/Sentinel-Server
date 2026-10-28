//! Tests de bout en bout contre un vrai PostgreSQL (base de test vide) :
//! `SENTINEL_TEST_DATABASE=postgres://… cargo test -- --ignored --test-threads=1`

use crate::state::AppState;
use crate::{auth, config, db, routes, sync};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use ed25519_dalek::{Signer, SigningKey};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;
use tower::ServiceExt;

async fn setup() -> (Router, Arc<AppState>) {
    setup_with("http://localhost:8080", Default::default()).await
}

async fn setup_with(public_url: &str, proxy: crate::proxy::ProxyConfig) -> (Router, Arc<AppState>) {
    let url = std::env::var("SENTINEL_TEST_DATABASE").expect("SENTINEL_TEST_DATABASE non défini");
    let pool = db::connect(&url, 4).await.unwrap();
    // Base de test : on repart de zéro.
    let tables: Vec<String> = sync::tables::TABLES.iter().map(|t| t.name.to_string()).collect();
    sqlx::query(&format!(
        "TRUNCATE users, devices, admin_codes, admin_sessions, audit_log, server_state, {} CASCADE",
        tables.join(", ")
    ))
    .execute(&pool)
    .await
    .unwrap();
    let cfg = config::Config {
        database_url: url,
        bind: "127.0.0.1:0".parse().unwrap(),
        public_url: public_url.into(),
        base_path: crate::proxy::base_path(public_url).unwrap(),
        token_ttl_days: 30,
        proxy,
        db_max_connections: 4,
    };
    let (head_tx, head_rx) = watch::channel(db::current_head(&pool).await.unwrap());
    let writer = sync::writer::Writer::spawn(pool.clone(), head_tx);
    let state = Arc::new(AppState {
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
    (routes::router(state.clone()), state)
}

/// Crée un compte avec un appareil (validé, sauf compte en attente) et renvoie son jeton.
async fn user(state: &AppState, id: &str, status: &str, role: &str) -> String {
    sqlx::query("INSERT INTO users (vrc_id, display_name, status, role, created_at) VALUES ($1, $2, $3, $4, 0)")
        .bind(id)
        .bind(format!("Pseudo {id}"))
        .bind(status)
        .bind(role)
        .execute(&state.pool)
        .await
        .unwrap();
    let device_status = if status == "pending" { "pending" } else { "active" };
    let device: i64 = sqlx::query_scalar(
        "INSERT INTO devices (user_id, public_key, fingerprint, name, status, created_at, last_used_at)
         VALUES ($1, $2, $3, 'test', $4, 0, 0) RETURNING id",
    )
    .bind(id)
    .bind(format!("cle-{id}"))
    .bind(format!("FP-{id}"))
    .bind(device_status)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let mut tx = state.pool.begin().await.unwrap();
    let token = auth::tokens::issue(&mut tx, device, 30).await.unwrap();
    tx.commit().await.unwrap();
    token
}

/// Connexion signée, comme le fait l'appli.
async fn sign_in(app: &Router, key: &SigningKey, user_id: &str, server: &str) -> (StatusCode, Value) {
    let (_, challenge, _) = call(app, "POST", "/api/v1/auth/challenge", None, None).await;
    let nonce = challenge["nonce"].as_str().unwrap().to_string();
    let public_key = hex::encode(key.verifying_key().as_bytes());
    let message = auth::device::login_message(server, &nonce, user_id, &public_key);
    let signature = hex::encode(key.sign(message.as_bytes()).to_bytes());
    let body = json!({
        "nonce": nonce, "user_id": user_id, "display_name": "Nouveau", "public_key": public_key,
        "signature": signature, "device": "PC de test",
    });
    let (status, value, _) = call(app, "POST", "/api/v1/auth/login", None, Some(body)).await;
    (status, value)
}

async fn call(app: &Router, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value, Vec<String>) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = match body {
        Some(body) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_string))
        .collect();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), cookies)
}

fn columns(table: &str) -> Vec<&'static str> {
    sync::tables::table(table).unwrap().column_names()
}

#[tokio::test]
#[ignore]
async fn sync_round_trip_with_permissions() {
    let (app, state) = setup().await;
    let a = user(&state, "usr_a", "active", "moderator").await;
    let b = user(&state, "usr_b", "active", "moderator").await;
    let pending = user(&state, "usr_p", "pending", "moderator").await;

    // A vote en son nom (accepté), et au nom de B (refusé) ; il signale un joueur.
    let push = json!({ "tables": {
        "votes": { "columns": columns("votes"), "rows": [
            ["player", "usr_x", "usr_a", "faux", -1, 100],
            ["player", "usr_x", "usr_b", null, -1, 100]
        ]},
        "reports": { "columns": columns("reports"), "rows": [
            ["rep_1", "usr_x", "X", "soundboard", "bruit", null, null, "usr_a", "A", 100, 0, 100, 2_000_000_000]
        ]},
        "avatars": { "columns": columns("avatars"), "rows": [
            ["Robot", null, null, "2026-10-01T10:00:00", "2026-10-02T10:00:00", 3, null, "{}", null, null, null, null, null, null, null, null, null, null, null, null, 100],
            ["Robot", null, null, "2026-09-01T10:00:00", "2026-10-01T10:00:00", 5, null, null, null, null, null, null, null, null, null, null, null, null, null, "Auteur", 101]
        ]}
    }});
    let (status, result, _) = call(&app, "POST", "/api/v1/sync/push", Some(&a), Some(push)).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!((result["accepted"].as_u64(), result["rejected"].as_u64()), (Some(4), Some(1)), "{result}");
    let head = result["head"].as_i64().unwrap();
    assert!(head > 0);

    // B relit tout : pseudo vérifié, avatar fusionné (deux versions dans le même envoi).
    let (status, pulled, _) = call(&app, "POST", "/api/v1/sync/pull", Some(&b), Some(json!({ "cursors": {} }))).await;
    assert_eq!(status, StatusCode::OK, "{pulled}");
    assert_eq!(pulled["head"].as_i64(), Some(head));
    let vote = &pulled["tables"]["votes"]["rows"][0];
    assert_eq!(vote[3], json!("Pseudo usr_a"), "pseudo réécrit par le serveur");
    let robot = &pulled["tables"]["avatars"]["rows"][0];
    assert_eq!(robot[3], json!("2026-09-01T10:00:00"), "première vue : la plus ancienne");
    assert_eq!(robot[5], json!(5), "compteur : le plus grand");
    assert_eq!(robot[7], json!("{}"), "réponse API gardée");
    assert_eq!(robot[19], json!("Auteur"));

    // Curseurs : plus rien à relire.
    let mut cursors = serde_json::Map::new();
    for (name, table) in pulled["tables"].as_object().unwrap() {
        cursors.insert(name.clone(), json!(table["seqs"].as_array().unwrap().last().unwrap()));
    }
    let (_, again, _) = call(&app, "POST", "/api/v1/sync/pull", Some(&b), Some(json!({ "cursors": cursors }))).await;
    assert!(again["tables"].as_object().unwrap().is_empty(), "{again}");

    // B ne peut pas modifier (ni supprimer) le signalement de A.
    let hijack = json!({ "tables": { "reports": { "columns": columns("reports"), "rows": [
        ["rep_1", "usr_x", "X", "other", null, null, null, "usr_b", "B", 100, 1, 200, 0]
    ]}}});
    let (_, result, _) = call(&app, "POST", "/api/v1/sync/push", Some(&b), Some(hijack)).await;
    assert_eq!(result["accepted"].as_u64(), Some(0), "{result}");
    let deleted: i64 = sqlx::query_scalar("SELECT deleted FROM reports WHERE id = 'rep_1'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(deleted, 0);

    // Compte en attente : identifié, mais pas de synchro.
    let (status, me, _) = call(&app, "GET", "/api/v1/auth/me", Some(&pending), None).await;
    assert_eq!((status, me["user"]["status"].as_str()), (StatusCode::OK, Some("pending")));
    let (status, _, _) = call(&app, "POST", "/api/v1/sync/pull", Some(&pending), Some(json!({}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(&app, "POST", "/api/v1/sync/pull", Some("sst_faux"), Some(json!({}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Attente longue : réponse immédiate si une écriture dépasse le curseur.
    let (status, waited, _) = call(&app, "GET", "/api/v1/sync/wait?since=0", Some(&b), None).await;
    assert_eq!((status, waited["head"].as_i64()), (StatusCode::OK, Some(head)));

    // Présence en mémoire.
    let (status, online, _) = call(&app, "POST", "/api/v1/presence", Some(&a), Some(json!({ "world_name": "Monde", "user_count": 12 }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(online[0]["display_name"], json!("Pseudo usr_a"));

    // Déconnexion : le jeton ne marche plus.
    let (status, _, _) = call(&app, "POST", "/api/v1/auth/logout", Some(&a), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(&app, "GET", "/api/v1/auth/me", Some(&a), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore]
async fn admin_link_session_and_user_management() {
    let (app, state) = setup().await;
    let admin = user(&state, "usr_admin", "active", "admin").await;
    let modo = user(&state, "usr_m", "active", "moderator").await;
    user(&state, "usr_new", "pending", "moderator").await;

    // Un modo ne peut pas obtenir de lien d'administration.
    let (status, _, _) = call(&app, "POST", "/api/v1/admin/link", Some(&modo), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, link, _) = call(&app, "POST", "/api/v1/admin/link", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let url = link["url"].as_str().unwrap();
    let path = url.trim_start_matches("http://localhost:8080");

    // Le lien ouvre une session (cookie verrouillé) et ne sert qu'une fois.
    let (status, _, cookies) = call(&app, "GET", path, None, None).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let cookie = cookies[0].split(';').next().unwrap().to_string();
    assert!(cookies[0].contains("HttpOnly") && cookies[0].contains("SameSite=Strict"));
    let (_, _, reused) = call(&app, "GET", path, None, None).await;
    assert!(reused.is_empty(), "lien déjà utilisé");

    let admin_call = |method: &'static str, uri: String, body: Option<Value>, csrf: bool| {
        let app = app.clone();
        let cookie = cookie.clone();
        async move {
            let mut request = Request::builder().method(method).uri(uri).header(header::COOKIE, cookie);
            if csrf {
                request = request.header("x-sentinel", "1");
            }
            let request = match body {
                Some(b) => request.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())),
                None => request.body(Body::empty()),
            }
            .unwrap();
            let response = app.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null))
        }
    };

    let (status, overview) = admin_call("GET", "/api/v1/admin/overview".into(), None, false).await;
    assert_eq!(status, StatusCode::OK, "{overview}");
    assert_eq!(overview["users"]["pending"].as_i64(), Some(1));

    // Sans l'en-tête anti-CSRF, aucune modification.
    let (status, _) = admin_call("POST", "/api/v1/admin/users/usr_new/status".into(), Some(json!({ "status": "active" })), false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = admin_call("POST", "/api/v1/admin/users/usr_new/status".into(), Some(json!({ "status": "active" })), true).await;
    assert_eq!(status, StatusCode::OK);
    let (_, users) = admin_call("GET", "/api/v1/admin/users?status=pending".into(), None, false).await;
    assert!(users["users"].as_array().unwrap().is_empty());

    // Bannir révoque aussitôt les jetons du modo.
    let (status, _) = admin_call("POST", "/api/v1/admin/users/usr_m/status".into(), Some(json!({ "status": "banned" })), true).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(&app, "GET", "/api/v1/auth/me", Some(&modo), None).await;
    assert_ne!(status, StatusCode::OK);

    // Pas d'auto-bannissement ; tables et journal lisibles.
    let (status, _) = admin_call("POST", "/api/v1/admin/users/usr_admin/status".into(), Some(json!({ "status": "banned" })), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, tables) = admin_call("GET", "/api/v1/admin/tables".into(), None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tables["tables"].as_array().unwrap().len(), sync::tables::TABLES.len());
    let (status, rows) = admin_call("GET", "/api/v1/admin/tables/votes?q=usr".into(), None, false).await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let (_, audit) = admin_call("GET", "/api/v1/admin/audit".into(), None, false).await;
    assert!(audit["entries"].as_array().unwrap().iter().any(|e| e["action"] == "user.banned"));
}

#[tokio::test]
#[ignore]
async fn device_login_needs_a_signature_and_an_admin() {
    let (app, state) = setup().await;
    let admin = user(&state, "usr_admin", "active", "admin").await;
    let key = SigningKey::from_bytes(&[9u8; 32]);
    let server = "http://localhost:8080";

    // Signature pour un autre serveur : refusée.
    let (status, _) = sign_in(&app, &key, "usr_new", "https://autre.exemple.fr").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Nouvel appareil : jeton délivré, mais compte et appareil en attente.
    let (status, login) = sign_in(&app, &key, "usr_new", server).await;
    assert_eq!(status, StatusCode::OK, "{login}");
    assert_eq!(login["user"]["status"], json!("pending"));
    let code = login["user"]["fingerprint"].as_str().unwrap().to_string();
    assert_eq!(code, crate::util::fingerprint(key.verifying_key().as_bytes()));
    let token = login["token"].as_str().unwrap().to_string();
    let (status, _, _) = call(&app, "POST", "/api/v1/sync/pull", Some(&token), Some(json!({}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "pas de données avant validation");

    // Le même compte annoncé depuis une autre clé : un deuxième appareil, lui aussi en attente.
    let intruder = SigningKey::from_bytes(&[3u8; 32]);
    let (_, other) = sign_in(&app, &intruder, "usr_new", server).await;
    assert_eq!(other["user"]["status"], json!("pending"));

    // L'admin valide l'appareil dont le code correspond.
    let device: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE fingerprint = $1")
        .bind(&code)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let (_, link) = call(&app, "POST", "/api/v1/admin/link", Some(&admin), None).await.into_admin_link();
    let (_, _, cookies) = call(&app, "GET", &link, None, None).await;
    let cookie = cookies[0].split(';').next().unwrap().to_string();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/admin/devices/{device}/approve"))
        .header(header::COOKIE, &cookie)
        .header("x-sentinel", "1")
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(request).await.unwrap().status(), StatusCode::OK);

    // Reconnexion avec la même clé : actif ; l'intrus reste en attente.
    let (_, login) = sign_in(&app, &key, "usr_new", server).await;
    assert_eq!(login["user"]["status"], json!("active"), "{login}");
    let token = login["token"].as_str().unwrap().to_string();
    let (status, _, _) = call(&app, "POST", "/api/v1/sync/pull", Some(&token), Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, other) = sign_in(&app, &intruder, "usr_new", server).await;
    assert_eq!(other["user"]["status"], json!("pending"));

    // Une clé liée à un compte ne peut pas se connecter pour un autre.
    let (status, _) = sign_in(&app, &key, "usr_admin", server).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Déconnexion : appareil révoqué, il ne peut plus se reconnecter tel quel.
    let (status, _, _) = call(&app, "POST", "/api/v1/auth/logout", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = sign_in(&app, &key, "usr_new", server).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

trait AdminLink {
    fn into_admin_link(self) -> (StatusCode, String);
}

impl AdminLink for (StatusCode, Value, Vec<String>) {
    fn into_admin_link(self) -> (StatusCode, String) {
        let url = self.1["url"].as_str().unwrap_or_default();
        (self.0, url.trim_start_matches("http://localhost:8080").to_string())
    }
}

#[tokio::test]
#[ignore]
async fn behind_a_reverse_proxy_under_a_sub_path() {
    // Requêtes de test sans adresse de connexion (0.0.0.0) : ce « proxy » est de confiance.
    let proxy = crate::proxy::ProxyConfig {
        trusted: vec![crate::proxy::Cidr::parse("0.0.0.0/32").unwrap()],
        header: "x-forwarded-for".into(),
    };
    let (app, state) = setup_with("https://exemple.fr/sentinel", proxy).await;
    let admin = user(&state, "usr_admin", "active", "admin").await;

    // Préfixe transmis ou retiré par le proxy : les deux fonctionnent.
    for path in ["/sentinel/health", "/health"] {
        let (status, body, _) = call(&app, "GET", path, None, None).await;
        assert_eq!((status, body["ok"].as_bool()), (StatusCode::OK, Some(true)), "{path}");
    }
    let request = Request::builder().uri("/sentinel/").body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers()[header::LOCATION], "/sentinel/admin/");
    let (status, _, _) = call(&app, "GET", "/sentinel/admin/", None, None).await;
    assert_eq!(status, StatusCode::OK);

    // Lien d'administration sous le chemin public ; cookie limité à ce chemin.
    let (_, link, _) = call(&app, "POST", "/sentinel/api/v1/admin/link", Some(&admin), None).await;
    let url = link["url"].as_str().unwrap();
    assert!(url.starts_with("https://exemple.fr/sentinel/admin/login?code="), "{url}");
    let path = url.trim_start_matches("https://exemple.fr");
    let request = Request::builder()
        .uri(path)
        .header("x-forwarded-for", "6.6.6.6, 203.0.113.9")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers()[header::LOCATION], "/sentinel/admin/");
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.contains("Path=/sentinel/;") && cookie.contains("Secure"), "{cookie}");

    // Adresse réelle du client : la plus à droite qui n'est pas un proxy, pas celle inventée.
    let ip: Option<String> = sqlx::query_scalar("SELECT detail FROM audit_log WHERE action = 'admin.login'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(ip.as_deref(), Some("203.0.113.9"));
}
