use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::Client;

const STATIONS_BASE: &str =
    "https://api1.raildata.org.uk/1010-knowlegebase-stations-xml-feed1_1/4.0";
const TOC_URL: &str = "https://api1.raildata.org.uk/1010-knowlegebase-toc-xml-feed2_0/4.0/tocs.xml";

pub struct StationRecord {
    pub crs_code: String,
    pub name: String,
    pub lat: f64,
    pub lng: f64,
    pub operator_code: String,
}

pub async fn fetch_and_seed(
    client: &Client,
    toc_api_key: &str,
    stations_api_key: &str,
    conn: &std::sync::Mutex<rusqlite::Connection>,
) -> Result<()> {
    tracing::info!("Fetching TOC feed from RDM…");
    let toc_xml = fetch_xml(client, TOC_URL, toc_api_key)
        .await
        .context("TOC feed request failed")?;
    let tocs = parse_tocs(&toc_xml).context("TOC XML parse failed")?;
    tracing::info!("Parsed {} TOCs", tocs.len());

    let mut all_stations: Vec<StationRecord> = Vec::new();

    for toc_code in tocs.keys() {
        let url = format!("{STATIONS_BASE}/stations-{toc_code}.xml");
        tracing::info!("Fetching stations for {toc_code}…");
        match fetch_xml(client, &url, stations_api_key).await {
            Ok(xml) => match parse_stations(&xml) {
                Ok(stations) => {
                    tracing::debug!("{} stations for {toc_code}", stations.len());
                    all_stations.extend(stations);
                }
                Err(e) => tracing::warn!("Parse failed for {toc_code}: {e}"),
            },
            Err(e) => tracing::warn!("Fetch failed for {toc_code}: {e}"),
        }
    }

    // Stations served by multiple TOCs appear more than once; keep first occurrence
    let mut seen = HashSet::new();
    let inserts: Vec<crate::db::StationInsert> = all_stations
        .into_iter()
        .filter(|s| seen.insert(s.crs_code.clone()))
        .map(|s| {
            let operator_name = tocs
                .get(&s.operator_code)
                .cloned()
                .unwrap_or_else(|| s.operator_code.clone());
            crate::db::StationInsert {
                crs_code: s.crs_code,
                name: s.name,
                lat: s.lat,
                lng: s.lng,
                operator_code: s.operator_code,
                operator_name,
            }
        })
        .collect();

    let inserted = inserts.len();
    tracing::info!("Inserting {inserted} unique stations into database…");
    let db = conn.lock().unwrap();
    crate::db::insert_stations(&db, &inserts).context("DB insert failed")?;
    tracing::info!("Seeded {inserted} stations");

    Ok(())
}

async fn fetch_xml(client: &Client, url: &str, api_key: &str) -> Result<String> {
    let resp = client
        .get(url)
        .header("x-apikey", api_key)
        .header("Accept", "application/xml, text/xml, */*")
        .send()
        .await?
        .error_for_status()?;

    let body = resp.text().await?;

    // Log a preview so parse errors are diagnosable
    tracing::debug!(
        "Response from {url} ({} bytes): {}…",
        body.len(),
        body.chars()
            .take(120)
            .collect::<String>()
            .replace('\n', " ")
    );

    Ok(body)
}

fn parse_stations(xml: &str) -> Result<Vec<StationRecord>> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut out = Vec::new();

    let mut depth: usize = 0;
    let mut station_depth: usize = 0;
    let mut in_station = false;
    let mut field = String::new();

    let mut crs = String::new();
    let mut name = String::new();
    let mut lat = 0f64;
    let mut lng = 0f64;
    let mut op = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                let tag = local_name(&e.name());
                if tag == "Station" {
                    in_station = true;
                    station_depth = depth;
                    crs.clear();
                    name.clear();
                    lat = 0.0;
                    lng = 0.0;
                    op.clear();
                } else if in_station && depth == station_depth + 1 {
                    field = tag;
                }
            }
            Ok(Event::Text(e)) => {
                if in_station && !field.is_empty() {
                    let text = e.unescape().unwrap_or_default().into_owned();
                    match field.as_str() {
                        "CrsCode" => crs = text,
                        "Name" => name = text,
                        "Latitude" => lat = text.trim().parse().unwrap_or(0.0),
                        "Longitude" => lng = text.trim().parse().unwrap_or(0.0),
                        "StationOperator" => op = text,
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let tag = local_name(&e.name());
                if in_station {
                    if depth == station_depth + 1 {
                        field.clear();
                    }
                    if tag == "Station" && depth == station_depth {
                        if !crs.is_empty() && (lat != 0.0 || lng != 0.0) {
                            out.push(StationRecord {
                                crs_code: crs.clone(),
                                name: name.clone(),
                                lat,
                                lng,
                                operator_code: op.clone(),
                            });
                        }
                        in_station = false;
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("XML error: {e}")),
            _ => {}
        }
        buf.clear();
    }

    Ok(out)
}

fn parse_tocs(xml: &str) -> Result<HashMap<String, String>> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut out = HashMap::new();

    let mut depth: usize = 0;
    let mut toc_depth: usize = 0;
    let mut in_toc = false;
    let mut field = String::new();
    let mut code = String::new();
    let mut toc_name = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                let tag = local_name(&e.name());
                if tag == "TrainOperatingCompany" {
                    in_toc = true;
                    toc_depth = depth;
                    code.clear();
                    toc_name.clear();
                } else if in_toc && depth == toc_depth + 1 {
                    field = tag;
                }
            }
            Ok(Event::Text(e)) => {
                if in_toc && !field.is_empty() {
                    let text = e.unescape().unwrap_or_default().into_owned();
                    match field.as_str() {
                        "AtocCode" => code = text,
                        "Name" => toc_name = text,
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let tag = local_name(&e.name());
                if in_toc {
                    if depth == toc_depth + 1 {
                        field.clear();
                    }
                    if tag == "TrainOperatingCompany" && depth == toc_depth {
                        if !code.is_empty() {
                            out.insert(code.clone(), toc_name.clone());
                        }
                        in_toc = false;
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("XML error: {e}")),
            _ => {}
        }
        buf.clear();
    }

    Ok(out)
}

fn local_name(name: &quick_xml::name::QName<'_>) -> String {
    String::from_utf8_lossy(name.local_name().as_ref()).into_owned()
}
