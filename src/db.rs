use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;

pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS stations (
            crs_code      TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            lat           REAL NOT NULL,
            lng           REAL NOT NULL,
            operator_code TEXT NOT NULL,
            operator_name TEXT NOT NULL,
            status        TEXT NOT NULL DEFAULT 'unvisited'
        );",
    )
}

pub fn is_empty(conn: &Connection) -> Result<bool> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM stations", [], |row| row.get(0))?;
    Ok(count == 0)
}

pub struct StationInsert {
    pub crs_code: String,
    pub name: String,
    pub lat: f64,
    pub lng: f64,
    pub operator_code: String,
    pub operator_name: String,
}

pub fn insert_stations(conn: &Connection, stations: &[StationInsert]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO stations
             (crs_code, name, lat, lng, operator_code, operator_name, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'unvisited')",
    )?;
    for s in stations {
        stmt.execute(params![
            s.crs_code,
            s.name,
            s.lat,
            s.lng,
            s.operator_code,
            s.operator_name
        ])?;
    }
    Ok(())
}

#[derive(Serialize)]
pub struct MarkerRow {
    pub crs: String,
    pub lat: f64,
    pub lng: f64,
    pub colour: String,
    pub name: String,
    pub operator_name: String,
    pub status: String,
}

pub fn get_markers(conn: &Connection) -> Result<Vec<MarkerRow>> {
    let mut stmt = conn.prepare(
        "SELECT crs_code, name, lat, lng, operator_name, status
         FROM stations
         ORDER BY name",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let status: String = row.get(5)?;
            Ok(MarkerRow {
                crs: row.get(0)?,
                name: row.get(1)?,
                lat: row.get(2)?,
                lng: row.get(3)?,
                operator_name: row.get(4)?,
                colour: status_colour(&status).to_string(),
                status,
            })
        })?
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn set_status(conn: &Connection, crs: &str, status: &str) -> Result<usize> {
    conn.execute(
        "UPDATE stations SET status = ?1 WHERE crs_code = ?2",
        params![status, crs.to_uppercase()],
    )
}

pub fn status_colour(status: &str) -> &'static str {
    match status {
        "visited" => "#f0c040",
        "done" => "#40c040",
        _ => "#4080f0",
    }
}

#[derive(Default, Serialize)]
pub struct RefreshStats {
    pub inserted: usize,
    pub updated: usize,
    pub crs_changed: usize,
}

// Upserts every station from the feed, preserving user-set statuses.
//
// Match order:
//   1. Exact CRS match  → update name/coords/operator, keep status
//   2. Location within ~200 m (no CRS match) → treat as CRS rename; update all fields, keep status
//   3. No match → insert as 'unvisited'
pub fn refresh_stations(conn: &Connection, stations: &[StationInsert]) -> Result<RefreshStats> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = refresh_inner(conn, stations);
    match &result {
        Ok(_) => conn.execute_batch("COMMIT")?,
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK");
        }
    }
    result
}

fn refresh_inner(conn: &Connection, stations: &[StationInsert]) -> Result<RefreshStats> {
    let mut stats = RefreshStats::default();

    let mut find_crs = conn.prepare("SELECT crs_code FROM stations WHERE crs_code = ?1")?;
    // 0.002° ≈ 220 m latitude / 140 m longitude in the UK — tight enough to
    // avoid false matches between adjacent stations, wide enough to survive
    // minor coordinate corrections in feed data.
    let mut find_nearby = conn.prepare(
        "SELECT crs_code FROM stations
         WHERE ABS(lat - ?1) < 0.002 AND ABS(lng - ?2) < 0.002
         LIMIT 1",
    )?;
    let mut update_fields = conn.prepare(
        "UPDATE stations
         SET name=?2, lat=?3, lng=?4, operator_code=?5, operator_name=?6
         WHERE crs_code=?1",
    )?;
    let mut update_crs = conn.prepare(
        "UPDATE stations
         SET crs_code=?1, name=?2, lat=?3, lng=?4, operator_code=?5, operator_name=?6
         WHERE crs_code=?7",
    )?;
    let mut insert = conn.prepare(
        "INSERT INTO stations (crs_code, name, lat, lng, operator_code, operator_name, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'unvisited')",
    )?;

    for s in stations {
        let existing: Option<String> = find_crs
            .query_row(params![s.crs_code], |row| row.get(0))
            .optional()?;

        if existing.is_some() {
            update_fields.execute(params![
                s.crs_code,
                s.name,
                s.lat,
                s.lng,
                s.operator_code,
                s.operator_name
            ])?;
            stats.updated += 1;
            continue;
        }

        let nearby_crs: Option<String> = find_nearby
            .query_row(params![s.lat, s.lng], |row| row.get(0))
            .optional()?;

        if let Some(old_crs) = nearby_crs {
            tracing::info!("CRS rename detected: {old_crs} → {}", s.crs_code);
            update_crs.execute(params![
                s.crs_code,
                s.name,
                s.lat,
                s.lng,
                s.operator_code,
                s.operator_name,
                old_crs
            ])?;
            stats.crs_changed += 1;
            continue;
        }

        insert.execute(params![
            s.crs_code,
            s.name,
            s.lat,
            s.lng,
            s.operator_code,
            s.operator_name
        ])?;
        stats.inserted += 1;
    }

    Ok(stats)
}
