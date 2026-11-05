//! Best-effort, extension-free browser history import.
//! Source files are ALWAYS opened SQLite read-only. No injected proxy, TLS
//! interception, browser add-on, or browser process modification is required.
//! All failures are isolated from habagent's foreground and Sysmon collectors.
use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CHROMIUM_EPOCH_US: i64 = 11_644_473_600_000_000;
const FIRST_SCAN_DAYS: i64 = 30;
const MAX_BATCH: i64 = 2_000;
const MAX_URL_CHARS: usize = 16_384;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum Browser { Firefox, Edge }

impl Browser {
    fn name(self) -> &'static str {
        match self { Self::Firefox => "firefox", Self::Edge => "edge" }
    }
    fn label(self) -> &'static str {
        match self { Self::Firefox => "Firefox", Self::Edge => "Edge" }
    }
    fn epoch_offset_us(self) -> i64 {
        if self == Self::Edge { CHROMIUM_EPOCH_US } else { 0 }
    }
    fn query(self) -> &'static str {
        match self {
            Self::Firefox =>
                "SELECT v.id, v.visit_date, p.url, p.title
                 FROM moz_historyvisits v
                 JOIN moz_places p ON p.id=v.place_id
                 WHERE (v.visit_date > ?1 OR (v.visit_date=?1 AND v.id > ?2))
                   AND v.visit_type NOT IN (4,5,6,7,8)
                 ORDER BY v.visit_date,v.id LIMIT ?3",
            Self::Edge =>
                "SELECT v.id, v.visit_time, u.url, u.title
                 FROM visits v
                 JOIN urls u ON u.id=v.url
                 WHERE (v.visit_time > ?1 OR (v.visit_time=?1 AND v.id > ?2))
                   AND (v.transition & 255) NOT IN (3,4)
                 ORDER BY v.visit_time,v.id LIMIT ?3",
        }
    }
}

#[derive(Debug)]
struct HistoryRow {
    id: i64,
    time_us: i64, // Native browser timestamp for cursor (includes browser epoch).
    url: String,
    title: Option<String>,
}

fn profile_key(path: &Path) -> String {
    let canonical = path.to_string_lossy().to_lowercase();
    let hash = Sha256::digest(canonical.as_bytes());
    format!("{:x}", hash)[..24].to_string()
}

fn sources(browser: Browser) -> Vec<PathBuf> {
    let base = match browser {
        Browser::Firefox => {
            let Some(path) = std::env::var_os("APPDATA") else { return Vec::new() };
            PathBuf::from(path).join("Mozilla").join("Firefox").join("Profiles")
        }
        Browser::Edge => {
            let Some(path) = std::env::var_os("LOCALAPPDATA") else { return Vec::new() };
            PathBuf::from(path).join("Microsoft").join("Edge").join("User Data")
        }
    };
    let Ok(entries) = std::fs::read_dir(base) else { return Vec::new() };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() { continue; }
        let basename = entry.file_name().to_string_lossy().to_string();
        if browser == Browser::Edge
            && basename != "Default"
            && !basename.starts_with("Profile ") {
            continue;
        }
        let history = path.join(if browser == Browser::Firefox { "places.sqlite" } else { "History" });
        if history.is_file() { result.push(history); }
    }
    result.sort();
    result
}

fn sanitize_url(raw: &str) -> Option<(String,String)> {
    if raw.chars().count() > MAX_URL_CHARS { return None; }
    let mut url = url::Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") { return None; }
    let host = url.host_str()?.to_ascii_lowercase();
    if !url.username().is_empty() && url.set_username("").is_err() { return None; }
    if url.password().is_some() && url.set_password(None).is_err() { return None; }
    Some((host,url.to_string()))
}

fn cursor_value(time_us: i64, id: i64) -> String {
    format!("{time_us}:{id}")
}

fn parse_cursor(value: &str) -> Result<(i64,i64)> {
    let (time,id) = value.rsplit_once(':').context("invalid browser history cursor")?;
    Ok((time.parse()?,id.parse()?))
}

// Reads a consistent SQLite snapshot while the browser is using WAL, when
// permitted. An OS file-sharing lock can still prevent opening the database.
fn read_rows(path: &Path, browser: Browser, after: (i64,i64)) -> Result<Vec<HistoryRow>> {
    let source = Connection::open_with_flags(
        path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    ).with_context(|| format!("open {} history read-only", browser.label()))?;
    source.busy_timeout(Duration::from_millis(250))?;
    let mut stmt = source.prepare(browser.query())
        .with_context(|| format!("query {} history tables", browser.label()))?;
    let rows = stmt.query_map(params![after.0,after.1,MAX_BATCH], |row| {
        Ok(HistoryRow {
            id: row.get(0)?,
            time_us: row.get(1)?,
            url: row.get(2)?,
            title: row.get(3)?,
        })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn import_profile(target: &mut Connection, path: &Path, browser: Browser, days: i64) -> Result<usize> {
    let key=profile_key(path);
    let cursor_key=format!("browser_history_cursor:{}:{}", browser.name(), key);
    let previous=crate::db::get_meta(target,&cursor_key)?;
    let cutoff_us = (Utc::now().timestamp() - days.clamp(1, 3650) * 86400)
        .saturating_mul(1_000_000).saturating_add(browser.epoch_offset_us());
    let mut checkpoint=previous.as_deref().map(parse_cursor).transpose()?
        .unwrap_or((cutoff_us,0));
    // Avoid replaying individual older history entries from an initial scan.
    let rows=read_rows(path,browser,checkpoint)?;
    let mut imported=0;
    let now_ms=Utc::now().timestamp_millis();
    let tx=target.transaction()?;
    for item in rows {
        checkpoint=(item.time_us,item.id);
        let Some(unix_us)=item.time_us.checked_sub(browser.epoch_offset_us()) else { continue };
        let ms=unix_us.div_euclid(1_000);
        if ms<=0 || ms > now_ms + 5 * 60_000 { continue; }
        let Some((host,url))=sanitize_url(&item.url) else { continue };
        let Some(datetime)=chrono::DateTime::<Utc>::from_timestamp_millis(ms) else { continue };
        let timestamp=datetime.to_rfc3339_opts(SecondsFormat::Millis,true);
        let title=item.title.map(|s|s.chars().filter(|c|!c.is_control()).take(2048).collect::<String>());
        imported += tx.execute(
            "INSERT OR IGNORE INTO browser_history_visits
             (browser,profile_key,source_visit_id,visited_ms,visited_utc,host,url,title)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![browser.name(),key,item.id,ms,timestamp,host,url,title],
        )?;
    }
    crate::db::set_meta(&tx,&cursor_key,&cursor_value(checkpoint.0,checkpoint.1))?;
    tx.commit()?;
    Ok(imported)
}

/// Returns an error only for target database failures. Source read failures are
/// treated as expected and reported through per-browser status metadata.
pub fn poll(target: &mut Connection, days: i64) -> Result<()> {
    for browser in [Browser::Firefox,Browser::Edge] {
        let paths=sources(browser);
        let mut imported=0_usize;
        let mut errors=Vec::new();
        for path in &paths {
            match import_profile(target,path,browser,days) {
                Ok(n) => imported += n,
                Err(error) => {
                    // Do not log a flood of identical collector errors every poll.
                    let reason=format!("{error:#}");
                    errors.push(reason.chars().take(250).collect::<String>());
                }
            }
        }
        let status=if paths.is_empty() {
            "no history profiles found".to_string()
        } else if errors.is_empty() {
            format!("ok ({} profiles, +{imported} new visits)",paths.len())
        } else {
            format!("limited ({}/{} profiles readable; {})",
                paths.len()-errors.len(),paths.len(),errors.join(" | "))
        };
        crate::db::set_meta(target,&format!("browser_history_{}_status",browser.name()),&status)?;
        crate::db::set_meta(target,&format!("browser_history_{}_last_poll",browser.name()),&crate::util::timestamp())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn firefox_import_is_incremental_and_preserves_page_title() {
        let dir=tempdir().unwrap();
        let path=dir.path().join("places.sqlite");
        let src=Connection::open(&path).unwrap();
        src.execute_batch(
            "CREATE TABLE moz_places(id INTEGER PRIMARY KEY,url TEXT,title TEXT);
             CREATE TABLE moz_historyvisits(id INTEGER PRIMARY KEY,place_id INTEGER,
                visit_date INTEGER,visit_type INTEGER);"
        ).unwrap();
        let t=Utc::now().timestamp_micros();
        src.execute("INSERT INTO moz_places VALUES(1,?1,'Example title')",
            ["https://someone:secret@example.org/page?q=interesting"]).unwrap();
        src.execute("INSERT INTO moz_historyvisits VALUES(1,1,?1,1)",[t]).unwrap();
        src.execute("INSERT INTO moz_historyvisits VALUES(2,1,?1,4)",[t+1_000]).unwrap();
        drop(src);
        let mut target=Connection::open_in_memory().unwrap();
        crate::db::init(&target).unwrap();
        assert_eq!(import_profile(&mut target,&path,Browser::Firefox,30).unwrap(),1);
        assert_eq!(import_profile(&mut target,&path,Browser::Firefox,30).unwrap(),0);
        let (url,title,host):(String,String,String)=target.query_row(
            "SELECT url,title,host FROM browser_history_visits",[],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))
        ).unwrap();
        assert_eq!(url,"https://example.org/page?q=interesting");
        assert_eq!(title,"Example title");
        assert_eq!(host,"example.org");
    }

    #[test]
    fn edge_import_converts_1601_epoch_and_skips_subframes() {
        let dir=tempdir().unwrap();
        let path=dir.path().join("History");
        let src=Connection::open(&path).unwrap();
        src.execute_batch(
            "CREATE TABLE urls(id INTEGER PRIMARY KEY,url TEXT,title TEXT);
             CREATE TABLE visits(id INTEGER PRIMARY KEY,url INTEGER,visit_time INTEGER,transition INTEGER);"
        ).unwrap();
        let t=Utc::now().timestamp_micros() + CHROMIUM_EPOCH_US;
        src.execute("INSERT INTO urls VALUES(1,'https://example.com/docs','Documentation')",[]).unwrap();
        src.execute("INSERT INTO visits VALUES(1,1,?1,0)",[t]).unwrap();
        src.execute("INSERT INTO visits VALUES(2,1,?1,3)",[t+1_000]).unwrap();
        drop(src);
        let mut target=Connection::open_in_memory().unwrap();
        crate::db::init(&target).unwrap();
        assert_eq!(import_profile(&mut target,&path,Browser::Edge,30).unwrap(),1);
        let ms:i64=target.query_row("SELECT visited_ms FROM browser_history_visits",[],|r|r.get(0)).unwrap();
        assert!((ms - Utc::now().timestamp_millis()).abs() < 5000);
    }
}
