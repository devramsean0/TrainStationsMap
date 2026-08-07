use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json, Response},
    routing::get,
    Router,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::fs;

const TILE_CACHE_DIR: &str = "./tile_cache";
const OSM_TILE_URL: &str = "https://tile.openstreetmap.org";

const UK_LAT_MIN: f64 = 49.8;
const UK_LAT_MAX: f64 = 60.9;
const UK_LON_MIN: f64 = -8.7;
const UK_LON_MAX: f64 = 1.9;

fn tile_in_uk(z: u32, x: u32, y: u32) -> bool {
    use std::f64::consts::PI;
    let n = (1u64 << z) as f64;
    let lon_min = x as f64 / n * 360.0 - 180.0;
    let lon_max = (x + 1) as f64 / n * 360.0 - 180.0;
    let lat_of_y = |row: u32| -> f64 {
        (PI * (1.0 - 2.0 * row as f64 / n))
            .sinh()
            .atan()
            .to_degrees()
    };
    let lat_max = lat_of_y(y);
    let lat_min = lat_of_y(y + 1);
    lat_max > UK_LAT_MIN && lat_min < UK_LAT_MAX && lon_max > UK_LON_MIN && lon_min < UK_LON_MAX
}

#[derive(Clone)]
struct AppState {
    client: Arc<Client>,
}

#[derive(Serialize, Deserialize)]
struct Marker {
    lat: f64,
    lng: f64,
    colour: String,
    label: Option<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    fs::create_dir_all(TILE_CACHE_DIR)
        .await
        .expect("Failed to create tile cache directory");

    tracing::info!("Tile cache directory ready at {TILE_CACHE_DIR}");

    let client = Client::builder()
        .user_agent("TrainStationsMap/0.1 (https://github.com/devramsean0/TrainStationsMap)")
        .build()
        .expect("Failed to build HTTP client");

    let state = AppState {
        client: Arc::new(client),
    };

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/tiles/:z/:x/:y", get(proxy_tile))
        .route("/api/markers", get(list_markers))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("Failed to bind to port 3000");

    tracing::info!("Server running at http://localhost:3000");
    axum::serve(listener, app).await.expect("Server error");
}

async fn serve_index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn proxy_tile(
    Path((z, x, y)): Path<(u32, u32, u32)>,
    State(state): State<AppState>,
) -> Response {
    if z > 19 || !tile_in_uk(z, x, y) {
        return StatusCode::NOT_FOUND.into_response();
    }

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

async fn list_markers() -> Json<Vec<Marker>> {
    Json(vec![])
}

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <title>UK Train Stations Map</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <link rel="stylesheet" href="https://unpkg.com/leaflet@1.9.4/dist/leaflet.css"
          integrity="sha256-20nQCchB9co0qIjJZRGuk2/Z9VM+kNiyxNV1lvTlZBo=" crossorigin=""/>
    <style>
        * { box-sizing: border-box; }
        body { margin: 0; padding: 0; font-family: sans-serif; }
        #map { width: 100vw; height: 100vh; }
    </style>
</head>
<body>
    <div id="map"></div>
    <script src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js"
            integrity="sha256-20nQCchB9co0qIjJZRGuk2/Z9VM+kNiyxNV1lvTlZBo=" crossorigin=""></script>
    <script>
        const ukBounds = L.latLngBounds(
            L.latLng(49.8, -8.7),
            L.latLng(60.9, 1.9)
        );

        const map = L.map('map', {
            maxBounds: ukBounds,
            maxBoundsViscosity: 1.0,
            minZoom: 5,
            maxZoom: 19,
        }).setView([54.5, -3.5], 6);

        L.tileLayer('/tiles/{z}/{x}/{y}', {
            attribution: '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors',
            minZoom: 5,
            maxZoom: 19,
            bounds: ukBounds,
        }).addTo(map);

        const markerIcons = {};

        function getColourIcon(colour) {
            if (!markerIcons[colour]) {
                markerIcons[colour] = L.divIcon({
                    className: '',
                    html: `<div style="
                        width: 16px;
                        height: 16px;
                        background-color: ${colour};
                        border: 2px solid rgba(0,0,0,0.5);
                        border-radius: 50%;
                        box-shadow: 0 1px 3px rgba(0,0,0,0.4);
                    "></div>`,
                    iconSize: [16, 16],
                    iconAnchor: [8, 8],
                    popupAnchor: [0, -10],
                });
            }
            return markerIcons[colour];
        }

        async function loadMarkers() {
            const resp = await fetch('/api/markers');
            const markers = await resp.json();
            for (const m of markers) {
                const marker = L.marker([m.lat, m.lng], { icon: getColourIcon(m.colour) });
                if (m.label) {
                    marker.bindPopup(m.label);
                }
                marker.addTo(map);
            }
        }

        loadMarkers();
    </script>
</body>
</html>"#;
