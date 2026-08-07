use rusqlite::{params, Connection, Result};
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
