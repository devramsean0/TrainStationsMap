mod db;
mod sync;

use std::sync::{Arc, Mutex};

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, patch, post},
    Router,
};
use reqwest::Client;
use rusqlite::Connection;
use serde::Deserialize;
use tokio::fs;
use tower_http::services::{ServeDir, ServeFile};

const TILE_CACHE_DIR: &str = "./tile_cache";
const OSM_TILE_URL: &str = "https://tile.openstreetmap.org";

#[derive(Clone)]
struct AppState {
    client: Arc<Client>,
    db: Arc<Mutex<Connection>>,
    auth_password: Option<String>,
}

#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

#[derive(Deserialize)]
struct StatusUpdate {
    status: StationStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum StationStatus {
    Unvisited,
    Visited,
    Done,
}

impl StationStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Unvisited => "unvisited",
            Self::Visited => "visited",
            Self::Done => "done",
        }
    }
}

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt::init();

    fs::create_dir_all(TILE_CACHE_DIR)
        .await
        .expect("Failed to create tile cache directory");

    let conn = Connection::open("stations.db").expect("Failed to open SQLite database");
    db::init(&conn).expect("Failed to initialise database schema");
    let db = Arc::new(Mutex::new(conn));

    let client = Client::builder()
        .user_agent("TrainStationsMap/0.1 (https://github.com/devramsean0/TrainStationsMap)")
        .build()
        .expect("Failed to build HTTP client");

    // Seed station data on first run
    {
        let is_empty = db::is_empty(&db.lock().unwrap()).expect("Failed to query station count");
        if is_empty {
            let toc_key = std::env::var("RDM_TOCS_API_KEY").unwrap_or_default();
            let stations_key = std::env::var("RDM_STATIONS_API_KEY").unwrap_or_default();
            if toc_key.is_empty() || stations_key.is_empty() {
                tracing::warn!(
                    "DB is empty but RDM_API_KEY and/or RDM_STATIONS_API_KEY are not set \
                     — skipping station sync. Set both and restart to populate station data."
                );
            } else {
                if let Err(e) = sync::fetch_and_seed(&client, &toc_key, &stations_key, &db).await {
                    tracing::error!("Station sync failed: {e:#}");
                }
            }
        }
    }

    let auth_password = std::env::var("AUTH_PASSWORD")
        .ok()
        .filter(|s| !s.is_empty());
    if auth_password.is_none() {
        tracing::warn!("AUTH_PASSWORD not set — status updates are unauthenticated");
    }

    let state = AppState {
        client: Arc::new(client),
        db,
        auth_password,
    };

    let frontend_dist =
        std::env::var("FRONTEND_DIST").unwrap_or_else(|_| "frontend/dist".to_string());
    let spa = ServeDir::new(&frontend_dist)
        .fallback(ServeFile::new(format!("{frontend_dist}/index.html")));

    let app = Router::new()
        .route("/api/tiles/:z/:x/:y", get(proxy_tile))
        .route("/api/markers", get(list_markers))
        .route("/api/stations/:crs/status", patch(update_station_status))
        .route("/api/auth/login", post(login))
        .route("/api/auth/verify", get(verify_auth))
        .route("/api/stations/refresh", post(station_refresh))
        .with_state(state)
        .fallback_service(spa);

    let listener = tokio::net::TcpListener::bind(
        std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string()),
    )
    .await
    .expect("Failed to bind to port 3000");

    tracing::info!("Server running at http://localhost:3000");
    axum::serve(listener, app).await.expect("Server error");
}

async fn verify_auth(State(state): State<AppState>, headers: HeaderMap) -> StatusCode {
    if check_auth(&state, &headers) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::UNAUTHORIZED
    }
}

async fn login(State(state): State<AppState>, Json(body): Json<LoginRequest>) -> impl IntoResponse {
    match &state.auth_password {
        None => StatusCode::OK.into_response(),
        Some(pw) if *pw == body.password => StatusCode::OK.into_response(),
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

fn check_auth(state: &AppState, headers: &HeaderMap) -> bool {
    match &state.auth_password {
        None => true,
        Some(pw) => headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|token| token == pw)
            .unwrap_or(false),
    }
}

async fn station_refresh(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if !check_auth(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let toc_key = std::env::var("RDM_TOCS_API_KEY").unwrap_or_default();
    let stations_key = std::env::var("RDM_STATIONS_API_KEY").unwrap_or_default();

    if toc_key.is_empty() || stations_key.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "RDM_TOCS_API_KEY and/or RDM_STATIONS_API_KEY not set",
        )
            .into_response();
    }

    let stations = match sync::fetch_stations(&state.client, &toc_key, &stations_key).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Station refresh fetch failed: {e:#}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || {
        let conn = db.lock().unwrap();
        db::refresh_stations(&conn, &stations)
    })
    .await
    {
        Ok(Ok(stats)) => {
            tracing::info!(
                "Station refresh: {} inserted, {} updated, {} CRS renamed",
                stats.inserted,
                stats.updated,
                stats.crs_changed,
            );
            Json(stats).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("DB refresh failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            tracing::error!("Task error during refresh: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn proxy_tile(
    Path((z, x, y)): Path<(u32, u32, u32)>,
    State(state): State<AppState>,
) -> Response {
    let cache_path = format!("{TILE_CACHE_DIR}/{z}/{x}/{y}.png");

    if let Ok(data) = fs::read(&cache_path).await {
        return (StatusCode::OK, [(header::CONTENT_TYPE, "image/png")], data).into_response();
    }

    let url = format!("{OSM_TILE_URL}/{z}/{x}/{y}.png");
    let resp = match state.client.get(&url).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return StatusCode::BAD_GATEWAY.into_response(),
    };

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if let Some(parent) = std::path::Path::new(&cache_path).parent() {
        let _ = fs::create_dir_all(parent).await;
    }
    let _ = fs::write(&cache_path, &bytes).await;

    tracing::debug!("Cached tile {z}/{x}/{y}");

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/png")],
        bytes.to_vec(),
    )
        .into_response()
}

async fn list_markers(State(state): State<AppState>) -> Response {
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || {
        let conn = db.lock().unwrap();
        db::get_markers(&conn)
    })
    .await
    {
        Ok(Ok(markers)) => Json(markers).into_response(),
        Ok(Err(e)) => {
            tracing::error!("DB error listing markers: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            tracing::error!("Task error listing markers: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn update_station_status(
    Path(crs): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<StatusUpdate>,
) -> StatusCode {
    if !check_auth(&state, &headers) {
        return StatusCode::UNAUTHORIZED;
    }
    let db = state.db.clone();
    let status = body.status.as_str().to_string();

    match tokio::task::spawn_blocking(move || {
        let conn = db.lock().unwrap();
        db::set_status(&conn, &crs, &status)
    })
    .await
    {
        Ok(Ok(0)) => StatusCode::NOT_FOUND,
        Ok(Ok(_)) => StatusCode::NO_CONTENT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
