//! Windows WebExtension native messaging host. Only receives sanitized domain events.
use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::json;
use std::io::{Read, Write};

const MAX_MESSAGE: usize = 64 * 1024;
const MAX_GAP_MS: i64 = 45_000;
const MAX_AGE_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserMessage {
    id: String,
    browser: String,
    kind: String,
    #[serde(default)]
    host: Option<String>,
    timestamp_ms: i64,
    #[serde(default)]
    tab_id: Option<i64>,
    #[serde(default)]
    window_id: Option<i64>,
}

fn validated_host(value: &str) -> Result<String> {
    let normalized = value.trim_end_matches('.').to_ascii_lowercase();
    anyhow::ensure!(!normalized.is_empty() && normalized.len() <= 253, "invalid host length");
    // Browsers send URL.hostname, not the full URL. Reject paths, credentials,
    // query strings, whitespace, and control characters at the native boundary.
    if normalized.starts_with('[') && normalized.ends_with(']') {
        anyhow::ensure!(
            normalized[1..normalized.len() - 1].parse::<std::net::Ipv6Addr>().is_ok(),
            "invalid IPv6 host"
        );
    } else {
        for label in normalized.split('.') {
            anyhow::ensure!(!label.is_empty() && label.len() <= 63, "invalid host label");
            anyhow::ensure!(
                label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && !label.starts_with('-') && !label.ends_with('-'),
                "invalid host characters"
            );
        }
    }
    Ok(normalized)
}

fn validate(event: &BrowserMessage) -> Result<Option<String>> {
    anyhow::ensure!(
        !event.id.is_empty() && event.id.len() <= 96
            && event.id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b)),
        "invalid event ID"
    );
    anyhow::ensure!(matches!(event.browser.as_str(), "firefox" | "edge"), "invalid browser");
    anyhow::ensure!(
        matches!(event.kind.as_str(), "visit" | "focus" | "heartbeat" | "blur"),
        "invalid event type"
    );
    let now = Utc::now().timestamp_millis();
    anyhow::ensure!(event.timestamp_ms >= now - MAX_AGE_MS && event.timestamp_ms <= now + 300_000,
        "event timestamp outside accepted range");
    let host = event.host.as_deref().map(validated_host).transpose()?;
    if event.kind != "blur" {
        anyhow::ensure!(host.is_some(), "event requires host");
    }
    Ok(host)
}

fn utc(ms: i64) -> Result<String> {
    Ok(chrono::DateTime::<Utc>::from_timestamp_millis(ms)
        .context("invalid timestamp")?
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn apply(conn: &mut Connection, event: &BrowserMessage) -> Result<()> {
    let host = validate(event)?;
    let tx = conn.transaction()?;
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO browser_seen_events(event_id,created_ms) VALUES(?1,?2)",
        params![event.id, event.timestamp_ms],
    )?;
    if inserted == 0 {
        tx.commit()?;
        return Ok(());
    }
    if event.kind == "visit" {
        tx.execute(
            "INSERT INTO browser_visits(event_id,browser,host,visited_utc,tab_id,window_id)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![event.id, event.browser, host, utc(event.timestamp_ms)?,
                event.tab_id, event.window_id],
        )?;
    } else {
        let previous: Option<(String, i64, i64)> = tx.query_row(
            "SELECT host, interval_id, last_ms FROM browser_focus_state WHERE browser=?1",
            [&event.browser],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?;
        if let Some((old_host, interval_id, last_ms)) = previous {
            let until = if event.timestamp_ms >= last_ms {
                event.timestamp_ms.min(last_ms.saturating_add(MAX_GAP_MS))
            } else {
                last_ms
            };
            tx.execute(
                "UPDATE browser_attention_intervals SET ended_ms=MAX(ended_ms,?1) WHERE id=?2",
                params![until, interval_id],
            )?;
            let continue_existing = event.kind != "blur"
                && host.as_deref() == Some(old_host.as_str())
                && event.timestamp_ms >= last_ms
                && event.timestamp_ms - last_ms <= MAX_GAP_MS;
            if continue_existing {
                tx.execute(
                    "UPDATE browser_focus_state SET last_ms=?1,tab_id=?2,window_id=?3 WHERE browser=?4",
                    params![event.timestamp_ms, event.tab_id, event.window_id, event.browser],
                )?;
                tx.commit()?;
                return Ok(());
            }
        }
        tx.execute("DELETE FROM browser_focus_state WHERE browser=?1", [&event.browser])?;
        if event.kind != "blur" {
            let host = host.expect("validated focus host");
            tx.execute(
                "INSERT INTO browser_attention_intervals(browser,host,tab_id,window_id,started_ms,ended_ms)
                 VALUES(?1,?2,?3,?4,?5,?5)",
                params![event.browser, host, event.tab_id, event.window_id, event.timestamp_ms],
            )?;
            let interval_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO browser_focus_state(browser,host,interval_id,last_ms,tab_id,window_id)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![event.browser, host, interval_id, event.timestamp_ms,
                    event.tab_id, event.window_id],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn read_frame<R: Read>(input: &mut R) -> Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    if input.read(&mut len[..1])? == 0 { return Ok(None); }
    input.read_exact(&mut len[1..])?;
    let length = u32::from_le_bytes(len) as usize;
    anyhow::ensure!(length > 0 && length <= MAX_MESSAGE, "invalid native message size");
    let mut payload = vec![0u8; length];
    input.read_exact(&mut payload)?;
    Ok(Some(payload))
}

fn write_frame<W: Write>(output: &mut W, data: &[u8]) -> Result<()> {
    anyhow::ensure!(data.len() <= MAX_MESSAGE, "native response too long");
    output.write_all(&(data.len() as u32).to_le_bytes())?;
    output.write_all(data)?;
    output.flush()?;
    Ok(())
}

fn serve<R: Read, W: Write>(input: &mut R, output: &mut W, conn: &mut Connection) -> Result<()> {
    while let Some(payload) = read_frame(input)? {
        // Invalid payloads still get a response, so the extension can discard
        // that item rather than blocking its entire durable queue.
        let response = match serde_json::from_slice::<BrowserMessage>(&payload) {
            Ok(message) => match apply(conn, &message) {
                Ok(()) => json!({"id":message.id,"ok":true}),
                Err(error) => json!({"id":message.id,"ok":false,"error":error.to_string()}),
            },
            Err(error) => json!({"id":null,"ok":false,"error":format!("invalid JSON: {error}")}),
        };
        write_frame(output, &serde_json::to_vec(&response)?)?;
    }
    Ok(())
}

pub fn run() -> Result<()> {
    let path = crate::util::database_path()?;
    let mut conn = crate::db::open(&path).with_context(|| format!("browser native host: {}", path.display()))?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve(&mut stdin.lock(), &mut stdout.lock(), &mut conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, kind: &str, host: Option<&str>, time: i64) -> BrowserMessage {
        BrowserMessage { id:id.into(), browser:"firefox".into(), kind:kind.into(),
            host:host.map(str::to_string), timestamp_ms: time, tab_id:Some(3),window_id:Some(1) }
    }

    #[test]
    fn visits_deduplicate_and_contain_only_host() {
        let mut db = Connection::open_in_memory().unwrap();
        crate::db::init(&db).unwrap();
        let t = Utc::now().timestamp_millis();
        let message = event("visit-one","visit",Some("Example.COM."),t);
        apply(&mut db,&message).unwrap();
        apply(&mut db,&message).unwrap();
        let (count,host):(i64,String) = db.query_row(
            "SELECT count(*), min(host) FROM browser_visits",[],|r|Ok((r.get(0)?,r.get(1)?))
        ).unwrap();
        assert_eq!(count,1);
        assert_eq!(host,"example.com");
        assert!(apply(&mut db,&event("bad","visit",Some("example.com/path"),t)).is_err());
    }

    #[test]
    fn attention_merges_heartbeats_and_caps_gaps() {
        let mut db = Connection::open_in_memory().unwrap();
        crate::db::init(&db).unwrap();
        let t = Utc::now().timestamp_millis();
        apply(&mut db,&event("f1","focus",Some("one.example"),t-120_000)).unwrap();
        apply(&mut db,&event("f2","heartbeat",Some("one.example"),t-100_000)).unwrap();
        apply(&mut db,&event("f3","focus",Some("two.example"),t-80_000)).unwrap();
        let total:i64 = db.query_row(
            "SELECT sum(ended_ms-started_ms) FROM browser_attention_intervals WHERE host='one.example'",
            [],|r|r.get(0)).unwrap();
        assert_eq!(total,40_000);
        apply(&mut db,&event("f4","blur",None,t)).unwrap();
        let two:i64 = db.query_row(
            "SELECT ended_ms-started_ms FROM browser_attention_intervals WHERE host='two.example'",
            [],|r|r.get(0)).unwrap();
        assert_eq!(two,45_000);
    }

    #[test]
    fn protocol_frames_round_trip() {
        let mut db = Connection::open_in_memory().unwrap();
        crate::db::init(&db).unwrap();
        let payload = serde_json::to_vec(&json!({
            "id":"a1","browser":"edge","kind":"visit","host":"example.org",
            "timestamp_ms":Utc::now().timestamp_millis(),"tab_id":5,"window_id":2
        })).unwrap();
        let mut wire = Vec::new();
        write_frame(&mut wire, &payload).unwrap();
        let mut output = Vec::new();
        serve(&mut &wire[..], &mut output, &mut db).unwrap();
        let reply = read_frame(&mut &output[..]).unwrap().unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&reply).unwrap()["ok"],true);
    }
}
