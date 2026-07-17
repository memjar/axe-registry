use axum::{
    Router, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Html},
    routing::{get, put},
    body::Bytes,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use tower_http::cors::CorsLayer;

const AXE_KEY: &str = "axe_bunker_2026_fleet";
const STORAGE: &str = "./storage";
const NPM_UPSTREAM: &str = "https://registry.npmjs.org";

struct AppState {
    db: Mutex<Connection>,
    http: reqwest::Client,
}

#[derive(Serialize, Deserialize)]
struct PackageMeta {
    name: String,
    version: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct SearchQuery {
    text: Option<String>,
    size: Option<usize>,
}

fn auth(headers: &HeaderMap) -> Result<(), StatusCode> {
    let key = headers.get("x-axe-key")
        .or_else(|| headers.get("authorization"))
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_start_matches("Bearer "))
        .unwrap_or("");
    if key == AXE_KEY { Ok(()) } else { Err(StatusCode::UNAUTHORIZED) }
}

fn init_db(db: &Connection) {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS packages (
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            description TEXT,
            shasum TEXT,
            tarball TEXT,
            keywords TEXT,
            created_at TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (name, version)
        )"
    ).unwrap();
}

fn normalize_scoped(name: &str) -> String {
    if name.starts_with("@") { name.replace("%2f", "/").replace("%2F", "/") } else { name.to_string() }
}

fn storage_dir(name: &str) -> PathBuf {
    PathBuf::from(STORAGE).join(name.replace("/", "__"))
}

async fn health() -> &'static str { "axe-pkg ok" }

async fn list_packages(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    let mut stmt = db.prepare(
        "SELECT DISTINCT name, version, description FROM packages ORDER BY created_at DESC"
    ).unwrap();
    let pkgs: Vec<PackageMeta> = stmt.query_map([], |row| {
        Ok(PackageMeta {
            name: row.get(0)?,
            version: row.get(1)?,
            description: row.get(2)?,
        })
    }).unwrap().filter_map(|r| r.ok()).collect();
    Json(serde_json::json!({ "packages": pkgs }))
}

async fn search(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
) -> impl IntoResponse {
    let text = q.text.unwrap_or_default();
    let size = q.size.unwrap_or(20).min(250);
    let db = state.db.lock().unwrap();
    let mut stmt = db.prepare(
        "SELECT DISTINCT name, MAX(version) as version, description FROM packages
         WHERE name LIKE ?1 OR description LIKE ?1 OR keywords LIKE ?1
         GROUP BY name ORDER BY created_at DESC LIMIT ?2"
    ).unwrap();
    let pattern = format!("%{}%", text);
    let results: Vec<serde_json::Value> = stmt.query_map(
        rusqlite::params![&pattern, size as i64],
        |row| {
            Ok(serde_json::json!({
                "package": {
                    "name": row.get::<_, String>(0)?,
                    "version": row.get::<_, String>(1)?,
                    "description": row.get::<_, Option<String>>(2)?,
                }
            }))
        },
    ).unwrap().filter_map(|r| r.ok()).collect();
    Json(serde_json::json!({ "objects": results, "total": results.len() }))
}

async fn get_package(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let name = normalize_scoped(&name);
    let db = state.db.lock().unwrap();
    let mut stmt = db.prepare(
        "SELECT version, description, shasum, tarball FROM packages WHERE name = ? ORDER BY created_at DESC"
    ).unwrap();
    let rows: Vec<(String, Option<String>, String, String)> = stmt.query_map([&name], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    }).unwrap().filter_map(|r| r.ok()).collect();

    if rows.is_empty() {
        return proxy_upstream(state.http.clone(), &name).await;
    }

    let mut versions = serde_json::Map::new();
    let mut latest = String::new();
    for (ver, desc, sha, tar) in &rows {
        if latest.is_empty() { latest = ver.clone(); }
        versions.insert(ver.clone(), serde_json::json!({
            "name": name,
            "version": ver,
            "description": desc,
            "dist": { "shasum": sha, "tarball": format!("https://pkg.axe.onl{}", tar) },
        }));
    }

    Json(serde_json::json!({
        "name": name,
        "dist-tags": { "latest": latest },
        "versions": versions,
    })).into_response()
}

async fn get_scoped_package(
    State(state): State<Arc<AppState>>,
    Path((scope, name)): Path<(String, String)>,
) -> impl IntoResponse {
    let full = format!("@{}/{}", scope, name);
    let db = state.db.lock().unwrap();
    let mut stmt = db.prepare(
        "SELECT version, description, shasum, tarball FROM packages WHERE name = ? ORDER BY created_at DESC"
    ).unwrap();
    let rows: Vec<(String, Option<String>, String, String)> = stmt.query_map([&full], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    }).unwrap().filter_map(|r| r.ok()).collect();

    if rows.is_empty() {
        return proxy_upstream(state.http.clone(), &full).await;
    }

    let mut versions = serde_json::Map::new();
    let mut latest = String::new();
    for (ver, desc, sha, tar) in &rows {
        if latest.is_empty() { latest = ver.clone(); }
        versions.insert(ver.clone(), serde_json::json!({
            "name": full,
            "version": ver,
            "description": desc,
            "dist": { "shasum": sha, "tarball": format!("https://pkg.axe.onl{}", tar) },
        }));
    }

    Json(serde_json::json!({
        "name": full,
        "dist-tags": { "latest": latest },
        "versions": versions,
    })).into_response()
}

async fn proxy_upstream(client: reqwest::Client, name: &str) -> axum::response::Response {
    let url = format!("{}/{}", NPM_UPSTREAM, name);
    match client.get(&url).header("accept", "application/json").send().await {
        Ok(resp) if resp.status().is_success() => {
            match resp.bytes().await {
                Ok(body) => (
                    StatusCode::OK,
                    [("content-type", "application/json")],
                    body,
                ).into_response(),
                Err(_) => StatusCode::BAD_GATEWAY.into_response(),
            }
        }
        _ => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"}))).into_response(),
    }
}

async fn npm_publish(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(e) = auth(&headers) {
        return (e, Json(serde_json::json!({"error":"unauthorized"}))).into_response();
    }

    let name = normalize_scoped(&name);

    if let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) {
        if let Some(versions) = payload.get("versions").and_then(|v| v.as_object()) {
            let attachments = payload.get("_attachments").and_then(|a| a.as_object());
            for (ver, meta) in versions {
                let desc = meta.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());
                let keywords = meta.get("keywords")
                    .and_then(|k| k.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(","));
                let sha = meta.pointer("/dist/shasum").and_then(|s| s.as_str()).unwrap_or("").to_string();
                let filename = format!("{}-{}.tgz", name.replace("/", "-"), ver);
                let tarball_path = format!("/{}/{}/-/{}", name, ver, filename);

                if let Some(atts) = attachments {
                    for (_att_name, att) in atts {
                        if let Some(data_str) = att.get("data").and_then(|d| d.as_str()) {
                            if let Ok(data) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data_str) {
                                let dir = storage_dir(&name);
                                std::fs::create_dir_all(&dir).unwrap();
                                std::fs::write(dir.join(&filename), &data).unwrap();
                            }
                        }
                    }
                }

                let db = state.db.lock().unwrap();
                let _ = db.execute(
                    "INSERT OR REPLACE INTO packages (name, version, description, shasum, tarball, keywords) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    (&name, ver, &desc, &sha, &tarball_path, &keywords),
                );
            }
            return Json(serde_json::json!({"ok": true, "name": name})).into_response();
        }
    }

    let version = headers.get("x-package-version")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("0.0.0").to_string();
    let description = headers.get("x-package-description")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let mut hasher = Sha256::new();
    hasher.update(&body);
    let shasum = hex::encode(hasher.finalize());

    let dir = storage_dir(&name);
    std::fs::create_dir_all(&dir).unwrap();
    let filename = format!("{}-{}.tgz", name.replace("/", "-"), version);
    std::fs::write(dir.join(&filename), &body).unwrap();

    let tarball = format!("/{}/{}/-/{}", name, version, filename);
    let db = state.db.lock().unwrap();
    let _ = db.execute(
        "INSERT OR REPLACE INTO packages (name, version, description, shasum, tarball) VALUES (?1, ?2, ?3, ?4, ?5)",
        (&name, &version, &description, &shasum, &tarball),
    );
    Json(serde_json::json!({"ok": true, "name": name, "version": version, "shasum": shasum})).into_response()
}

async fn npm_publish_scoped(
    state: State<Arc<AppState>>,
    headers: HeaderMap,
    Path((scope, name)): Path<(String, String)>,
    body: Bytes,
) -> impl IntoResponse {
    let full = format!("@{}/{}", scope, name);
    let mut h = headers.clone();
    npm_publish(state, h, Path(full), body).await
}

async fn download(
    Path((name, _version, filename)): Path<(String, String, String)>,
) -> impl IntoResponse {
    let path = storage_dir(&name).join(&filename);
    match std::fs::read(&path) {
        Ok(data) => (StatusCode::OK, [("content-type", "application/gzip")], data).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn download_scoped(
    Path((scope, name, version, filename)): Path<(String, String, String, String)>,
) -> impl IntoResponse {
    let full = format!("@{}/{}", scope, name);
    let path = storage_dir(&full).join(&filename);
    match std::fs::read(&path) {
        Ok(data) => (StatusCode::OK, [("content-type", "application/gzip")], data).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn web_ui(State(state): State<Arc<AppState>>) -> Html<String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db.prepare(
        "SELECT name, MAX(version), description, COUNT(*) FROM packages GROUP BY name ORDER BY MAX(created_at) DESC"
    ).unwrap();
    let rows: Vec<(String, String, Option<String>, i64)> = stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    }).unwrap().filter_map(|r| r.ok()).collect();

    let mut html = String::from(r#"<!DOCTYPE html><html><head><meta charset="utf-8">
<title>AXE Registry</title><style>
*{margin:0;padding:0;box-sizing:border-box}
body{background:#0a0a0a;color:#e0e0e0;font-family:'Space Grotesk',-apple-system,sans-serif}
.header{padding:32px;border-bottom:1px solid #1a1a1a;display:flex;align-items:center;gap:16px}
.header h1{font-size:20px;color:#D4AF37;letter-spacing:2px}
.header span{color:#666;font-size:12px}
.grid{padding:24px;display:grid;gap:12px}
.pkg{background:#111;border:1px solid #1a1a1a;border-radius:8px;padding:16px;display:flex;justify-content:space-between;align-items:center}
.pkg:hover{border-color:#D4AF37}
.pkg-name{font-family:'IBM Plex Mono',monospace;color:#D4AF37;font-size:14px}
.pkg-desc{color:#888;font-size:12px;margin-top:4px}
.pkg-ver{font-family:monospace;color:#666;font-size:12px}
.empty{text-align:center;padding:80px;color:#444}
</style></head><body>
<div class="header"><h1>AXE REGISTRY</h1><span>"#);
    html.push_str(&format!("{} packages", rows.len()));
    html.push_str(r#"</span></div><div class="grid">"#);

    if rows.is_empty() {
        html.push_str(r#"<div class="empty">No packages published yet</div>"#);
    }
    for (name, ver, desc, count) in &rows {
        html.push_str(&format!(
            r#"<div class="pkg"><div><div class="pkg-name">{}</div><div class="pkg-desc">{}</div></div><div class="pkg-ver">v{} ({} ver{})</div></div>"#,
            name,
            desc.as_deref().unwrap_or(""),
            ver,
            count,
            if *count != 1 { "s" } else { "" },
        ));
    }
    html.push_str("</div></body></html>");
    Html(html)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    std::fs::create_dir_all(STORAGE).unwrap();
    let db = Connection::open("axe-pkg.db").unwrap();
    init_db(&db);
    let http = reqwest::Client::new();
    let state = Arc::new(AppState { db: Mutex::new(db), http });

    let app = Router::new()
        .route("/", get(web_ui))
        .route("/health", get(health))
        .route("/packages", get(list_packages))
        .route("/-/v1/search", get(search))
        .route("/@{scope}/{name}", get(get_scoped_package))
        .route("/@{scope}/{name}", put(npm_publish_scoped))
        .route("/@{scope}/{name}/{version}/-/{filename}", get(download_scoped))
        .route("/{name}", get(get_package))
        .route("/{name}", put(npm_publish))
        .route("/{name}/{version}/-/{filename}", get(download))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = "0.0.0.0:8877";
    tracing::info!("axe-pkg listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
