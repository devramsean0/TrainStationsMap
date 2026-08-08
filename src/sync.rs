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

/// Fetches all stations from the RDM API and returns them deduplicated,
/// ready for either initial seeding or a refresh upsert.
pub async fn fetch_stations(
    client: &Client,
    toc_api_key: &str,
    stations_api_key: &str,
) -> Result<Vec<crate::db::StationInsert>> {
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

    // Stations served by multiple TOCs appear more than once; keep first occurrence.
    let mut seen = HashSet::new();
    let inserts = all_stations
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
        .collect::<Vec<_>>();

    tracing::info!("Fetched {} unique stations from API", inserts.len());
    Ok(inserts)
}

pub async fn fetch_and_seed(
    client: &Client,
    toc_api_key: &str,
    stations_api_key: &str,
    conn: &std::sync::Mutex<rusqlite::Connection>,
) -> Result<()> {
    let stations = fetch_stations(client, toc_api_key, stations_api_key).await?;
    let n = stations.len();
    tracing::info!("Inserting {n} stations into database…");
    let db = conn.lock().unwrap();
    crate::db::insert_stations(&db, &stations).context("DB insert failed")?;
    tracing::info!("Seeded {n} stations");
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

fn parse_xml_records<T, F>(xml: &str, record_tag: &str, mut handle: F) -> Result<Vec<T>>
where
    F: FnMut(&HashMap<String, String>) -> Option<T>,
{
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut out = Vec::new();

    let mut depth: usize = 0;
    let mut record_depth: usize = 0;
    let mut in_record = false;
    let mut field = String::new();
    let mut fields: HashMap<String, String> = HashMap::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                let tag = local_name(&e.name());
                if tag == record_tag {
                    in_record = true;
                    record_depth = depth;
                    fields.clear();
                } else if in_record && depth == record_depth + 1 {
                    field = tag;
                }
            }
            Ok(Event::Text(e)) => {
                if in_record && !field.is_empty() {
                    fields.insert(field.clone(), e.unescape().unwrap_or_default().into_owned());
                }
            }
            Ok(Event::End(e)) => {
                let tag = local_name(&e.name());
                if in_record {
                    if depth == record_depth + 1 {
                        field.clear();
                    }
                    if tag == record_tag && depth == record_depth {
                        if let Some(record) = handle(&fields) {
                            out.push(record);
                        }
                        in_record = false;
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

fn parse_stations(xml: &str) -> Result<Vec<StationRecord>> {
    parse_xml_records(xml, "Station", |fields| {
        let crs = fields.get("CrsCode").cloned().unwrap_or_default();
        let lat: f64 = fields
            .get("Latitude")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0.0);
        let lng: f64 = fields
            .get("Longitude")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0.0);
        if crs.is_empty() || (lat == 0.0 && lng == 0.0) {
            return None;
        }
        Some(StationRecord {
            crs_code: crs,
            name: fields.get("Name").cloned().unwrap_or_default(),
            lat,
            lng,
            operator_code: fields.get("StationOperator").cloned().unwrap_or_default(),
        })
    })
}

fn parse_tocs(xml: &str) -> Result<HashMap<String, String>> {
    let pairs = parse_xml_records(xml, "TrainOperatingCompany", |fields| {
        let code = fields.get("AtocCode").cloned().unwrap_or_default();
        if code.is_empty() {
            return None;
        }
        Some((code, fields.get("Name").cloned().unwrap_or_default()))
    })?;
    Ok(pairs.into_iter().collect())
}

fn local_name(name: &quick_xml::name::QName<'_>) -> String {
    String::from_utf8_lossy(name.local_name().as_ref()).into_owned()
}
