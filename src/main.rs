use axum::{
    Router, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
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

struct AppState {
    db: Mutex<Connection>,
}

#[derive(Serialize, Deserialize)]
struct PackageMeta {
    name: String,
    version: String,
    description: Option<String>,
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
            created_at TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (name, version)
        )"
    ).unwrap();
}

async fn health() -> &'static str { "axe-pkg ok" }

async fn list_packages(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    let mut stmt = db.prepare(
        "SELECT name, version, description FROM packages ORDER BY created_at DESC"
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

async fn get_package(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    let mut stmt = db.prepare(
        "SELECT name, version, description, shasum, tarball FROM packages WHERE name = ? ORDER BY created_at DESC"
    ).unwrap();
    let versions: Vec<serde_json::Value> = stmt.query_map([&name], |row| {
        Ok(serde_json::json!({
            "version": row.get::<_, String>(1)?,
            "description": row.get::<_, Option<String>>(2)?,
            "shasum": row.get::<_, String>(3)?,
            "tarball": row.get::<_, String>(4)?,
        }))
    }).unwrap().filter_map(|r| r.ok()).collect();
    if versions.is_empty() {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"}))).into_response();
    }
    Json(serde_json::json!({
        "name": name,
        "versions": versions,
        "latest": versions[0],
    })).into_response()
}

async fn publish(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(e) = auth(&headers) { return (e, Json(serde_json::json!({"error":"unauthorized"}))).into_response(); }

    let version = headers.get("x-package-version")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("0.0.0").to_string();
    let description = headers.get("x-package-description")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let mut hasher = Sha256::new();
    hasher.update(&body);
    let shasum = hex::encode(hasher.finalize());

    let dir = PathBuf::from(STORAGE).join(&name);
    std::fs::create_dir_all(&dir).unwrap();
    let filename = format!("{}-{}.tgz", name, version);
    let filepath = dir.join(&filename);
    std::fs::write(&filepath, &body).unwrap();

    let tarball = format!("/{}/{}/-/{}", name, version, filename);
    let db = state.db.lock().unwrap();
    match db.execute(
        "INSERT OR REPLACE INTO packages (name, version, description, shasum, tarball) VALUES (?1, ?2, ?3, ?4, ?5)",
        (&name, &version, &description, &shasum, &tarball),
    ) {
        Ok(_) => Json(serde_json::json!({
            "ok": true,
            "name": name,
            "version": version,
            "shasum": shasum,
        })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

async fn download(
    Path((name, _version, filename)): Path<(String, String, String)>,
) -> impl IntoResponse {
    let path = PathBuf::from(STORAGE).join(&name).join(&filename);
    match std::fs::read(&path) {
        Ok(data) => (
            StatusCode::OK,
            [("content-type", "application/gzip")],
            data,
        ).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    std::fs::create_dir_all(STORAGE).unwrap();
    let db = Connection::open("axe-pkg.db").unwrap();
    init_db(&db);
    let state = Arc::new(AppState { db: Mutex::new(db) });

    let app = Router::new()
        .route("/health", get(health))
        .route("/packages", get(list_packages))
        .route("/{name}", get(get_package))
        .route("/{name}", put(publish))
        .route("/{name}/{version}/-/{filename}", get(download))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = "0.0.0.0:8877";
    tracing::info!("axe-pkg listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
