use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use quick_xml::{events::Event, reader::Reader};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct SysmonEvent {
    pub record_id: i64,
    pub event_id: u32,
    pub timestamp_utc: String,
    pub data: HashMap<String, String>,
}

fn parse_time(value: &str) -> Result<String> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Millis, true));
    }
    // Sysmon EventData UtcTime uses "YYYY-MM-DD HH:MM:SS.mmm".
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, fmt) {
            return Ok(parsed.and_utc().to_rfc3339_opts(SecondsFormat::Millis, true));
        }
    }
    anyhow::bail!("unknown timestamp format: {value}")
}

pub fn parse_xml(xml: &str) -> Result<SysmonEvent> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut current: Option<String> = None;
    let mut record_id = None;
    let mut event_id = None;
    let mut timestamp = None;
    let mut data = HashMap::new();

    loop {
        match reader.read_event()? {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if name == "Data" {
                    current = e.attributes().flatten()
                        .find(|a| a.key.as_ref() == b"Name")
                        .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                } else if name == "EventID" || name == "EventRecordID" {
                    current = Some(name);
                }
            }
            Event::Empty(e) => {
                if e.local_name().as_ref() == b"TimeCreated" {
                    timestamp = e.attributes().flatten()
                        .find(|a| a.key.as_ref() == b"SystemTime")
                        .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                }
            }
            Event::Text(t) => {
                if let Some(field) = &current {
                    let raw = std::str::from_utf8(t.as_ref())?;
                    let decoded = quick_xml::escape::unescape(raw)?.into_owned();
                    match field.as_str() {
                        "EventID" => event_id = Some(decoded.parse::<u32>()?),
                        "EventRecordID" => record_id = Some(decoded.parse::<i64>()?),
                        _ => { data.insert(field.clone(), decoded); }
                    }
                }
            }
            Event::End(_) => { current = None; }
            Event::Eof => break,
            _ => {}
        }
    }
    let raw_time = timestamp.or_else(|| data.get("UtcTime").cloned())
        .context("Sysmon event lacks timestamp")?;
    Ok(SysmonEvent {
        record_id: record_id.context("Sysmon event lacks record ID")?,
        event_id: event_id.context("Sysmon event lacks event ID")?,
        timestamp_utc: parse_time(&raw_time)?,
        data,
    })
}

fn as_i64(event: &SysmonEvent, field: &str) -> Option<i64> {
    event.data.get(field).and_then(|s| s.parse().ok())
}
fn field<'a>(event: &'a SysmonEvent, field: &str) -> Option<&'a str> {
    event.data.get(field).map(String::as_str)
}

fn executable_name(event: &SysmonEvent) -> String {
    let path = field(event, "Image").unwrap_or("unknown");
    path.rsplit(['\\', '/']).next().unwrap_or("unknown").to_ascii_lowercase()
}
fn tracked_browser(app: &str) -> bool {
    matches!(app, "firefox.exe" | "msedge.exe")
}
fn dns_host(value: &str) -> Option<String> {
    let host=value.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 || host.contains('/') || host.contains('@')
        || host.contains(' ') { return None; }
    Some(host)
}

pub fn insert_events(conn: &mut Connection, events: &[SysmonEvent]) -> Result<()> {
    if events.is_empty() { return Ok(()); }
    let log_epoch: i64 = crate::db::get_meta(conn, "sysmon_epoch")?
        .as_deref().unwrap_or("0").parse().context("invalid Sysmon log epoch")?;
    // The checkpoint advances in the SAME transaction as the rollups.
    // This gives crash-safe exactly-once processing without persisting one
    // receipt per noisy raw network event (which would defeat aggregation).
    let checkpoint: i64 = crate::db::get_meta(conn,"sysmon_last_record_id")?
        .as_deref().unwrap_or("0").parse()?;
    let tx = conn.transaction()?;
    let mut latest: Option<i64> = None;
    let mut seen = HashSet::new();
    for ev in events {
        if ev.record_id <= checkpoint || !seen.insert(ev.record_id) { continue; }
        latest = Some(latest.unwrap_or(0).max(ev.record_id));
        match ev.event_id {
            1 | 5 => {
                tx.execute(r#"INSERT OR IGNORE INTO process_events
                    (log_epoch,record_id,timestamp_utc,event_id,process_guid,pid,image,parent_guid)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"#,
                    params![log_epoch, ev.record_id, ev.timestamp_utc, ev.event_id,
                        field(ev,"ProcessGuid"), as_i64(ev,"ProcessId"),
                        field(ev,"Image"), field(ev,"ParentProcessGuid")])?;
            }
            3 => {
                let app=executable_name(ev);
                // Browser browsing history is captured by the extension;
                // 3rd-party ad/telemetry subresource connections are noise.
                if tracked_browser(&app) { continue; }
                let ip=field(ev,"DestinationIp").unwrap_or("unknown");
                let port=as_i64(ev,"DestinationPort").unwrap_or(0);
                let protocol=field(ev,"Protocol").unwrap_or("unknown").to_ascii_lowercase();
                let hour=format!("{}:00:00.000Z",&ev.timestamp_utc[..13]);
                tx.execute(
                    r#"INSERT INTO app_connection_hours
                       (app,destination_ip,destination_port,protocol,hour_utc,first_utc,last_utc,connection_count)
                       VALUES(?1,?2,?3,?4,?5,?6,?6,1)
                       ON CONFLICT(app,destination_ip,destination_port,protocol,hour_utc)
                       DO UPDATE SET first_utc=MIN(first_utc,excluded.first_utc),
                                     last_utc=MAX(last_utc,excluded.last_utc),
                                     connection_count=connection_count+1"#,
                    params![app,ip,port,protocol,hour,ev.timestamp_utc],
                )?;
            }
            22 => {
                let app=executable_name(ev);
                if tracked_browser(&app) { continue; }
                let Some(host)=field(ev,"QueryName").and_then(dns_host) else { continue };
                let minute=format!("{}:00.000Z",&ev.timestamp_utc[..16]);
                tx.execute(
                    r#"INSERT INTO app_domain_minutes
                       (app,host,minute_utc,first_utc,last_utc,query_count)
                       VALUES(?1,?2,?3,?4,?4,1)
                       ON CONFLICT(app,host,minute_utc)
                       DO UPDATE SET first_utc=MIN(first_utc,excluded.first_utc),
                                     last_utc=MAX(last_utc,excluded.last_utc),
                                     query_count=query_count+1"#,
                    params![app,host,minute,ev.timestamp_utc],
                )?;
            }
            _ => {}
        }
    }
    if let Some(last) = latest {
        crate::db::set_meta(&tx, "sysmon_last_record_id", &last.to_string())?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(windows)]
mod native {
    use super::*;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS};
    use windows_sys::Win32::System::EventLog::{
        EvtClose, EvtNext, EvtQuery, EvtRender,
        EvtQueryChannelPath, EvtQueryForwardDirection, EvtQueryReverseDirection,
        EvtRenderEventXml, EVT_HANDLE,
    };

    const CHANNEL: &str = "Microsoft-Windows-Sysmon/Operational";
    const MAX_BATCH: usize = 1024;

    struct EvtHandle(EVT_HANDLE);
    impl Drop for EvtHandle {
        fn drop(&mut self) { if self.0 != 0 { unsafe { EvtClose(self.0); } } }
    }

    fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(std::iter::once(0)).collect() }

    fn query(xpath: &str, reverse: bool) -> Result<EvtHandle> {
        let channel = wide(CHANNEL);
        let query = wide(xpath);
        let dir = if reverse { EvtQueryReverseDirection } else { EvtQueryForwardDirection };
        // SAFETY: both UTF-16 buffers are valid for the duration of the call.
        let handle = unsafe { EvtQuery(0, channel.as_ptr(), query.as_ptr(), EvtQueryChannelPath | dir) };
        if handle == 0 { return Err(std::io::Error::last_os_error()).context("EvtQuery failed (is Sysmon installed and readable?)"); }
        Ok(EvtHandle(handle))
    }

    fn render(handle: EVT_HANDLE) -> Result<String> {
        let mut used = 0_u32;
        let mut property_count = 0_u32;
        // SAFETY: probing with a zero buffer is part of EvtRender API contract.
        let ok = unsafe { EvtRender(0, handle, EvtRenderEventXml, 0,
            std::ptr::null_mut(), &mut used, &mut property_count) };
        if ok != 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
            anyhow::bail!("EvtRender size probe: {}", std::io::Error::last_os_error());
        }
        let mut buffer = vec![0_u16; (used as usize).div_ceil(2)];
        // SAFETY: EvtRender returns UTF-16 bytes in the caller-provided buffer.
        if unsafe { EvtRender(0, handle, EvtRenderEventXml, used,
            buffer.as_mut_ptr().cast(), &mut used, &mut property_count) } == 0 {
            return Err(std::io::Error::last_os_error()).context("EvtRender failed");
        }
        let n = buffer.iter().position(|v| *v == 0).unwrap_or(buffer.len());
        Ok(String::from_utf16(&buffer[..n])?)
    }

    pub fn newest_record_id() -> Result<Option<i64>> {
        let query = query("*", true)?;
        let mut event = [0_isize; 1];
        let mut returned = 0_u32;
        // SAFETY: EvtNext writes at most one EVT_HANDLE to event.
        let ok = unsafe { EvtNext(query.0, 1, event.as_mut_ptr(), 0, 0, &mut returned) };
        if ok == 0 {
            let code = unsafe { GetLastError() };
            if code == ERROR_NO_MORE_ITEMS { return Ok(None); }
            return Err(std::io::Error::last_os_error()).context("checking newest Sysmon event");
        }
        if returned == 0 { return Ok(None); }
        let event_handle = EvtHandle(event[0]);
        Ok(Some(parse_xml(&render(event_handle.0)?)?.record_id))
    }

    pub fn collect(conn: &mut Connection) -> Result<usize> {
        let previous = crate::db::get_meta(conn, "sysmon_last_record_id")?;
        let mut last: i64 = if let Some(v) = previous {
            v.parse().context("invalid Sysmon record checkpoint")?
        } else {
            // First collection starts at the current tail. No surprising multi-year backfill.
            let newest = newest_record_id()?.unwrap_or(0);
            crate::db::set_meta(conn, "sysmon_last_record_id", &newest.to_string())?;
            return Ok(0);
        };
        // Handle cleared/recreated logs. A checkpoint greater than the newest record
        // would otherwise cause silent, permanent non-collection.
        if let Some(newest) = newest_record_id()? {
            if newest < last {
                crate::db::record_error(conn, "sysmon", "Sysmon log record IDs reset; possible log clear or event loss")?;
                let epoch: i64 = crate::db::get_meta(conn, "sysmon_epoch")?
                    .as_deref().unwrap_or("0").parse()?;
                crate::db::set_meta(conn, "sysmon_epoch", &(epoch + 1).to_string())?;
                crate::db::set_meta(conn, "sysmon_last_record_id", "0")?;
                last = 0;
            }
        }
        let xpath = format!("*[System[(EventID=1 or EventID=3 or EventID=5 or EventID=22) and (EventRecordID > {last})]]");
        let result = query(&xpath, false)?;
        let mut all = Vec::new();
        while all.len() < MAX_BATCH {
            let mut handles = [0_isize; 64];
            let mut count = 0_u32;
            let requested = (MAX_BATCH - all.len()).min(handles.len()) as u32;
            // SAFETY: handles has capacity for requested items, all handles are closed below.
            let ok = unsafe { EvtNext(result.0, requested, handles.as_mut_ptr(), 0, 0, &mut count) };
            if ok == 0 {
                let code = unsafe { GetLastError() };
                if code == ERROR_NO_MORE_ITEMS { break; }
                return Err(std::io::Error::last_os_error()).context("reading Sysmon events");
            }
            // Use RAII before parsing so errors cannot leak event handles.
            let batch: Vec<EvtHandle> = handles[..count as usize].iter().copied().map(EvtHandle).collect();
            for handle in batch {
                all.push(parse_xml(&render(handle.0)?)?);
            }
        }
        let count = all.len();
        insert_events(conn, &all)?;
        Ok(count)
    }
}

#[cfg(windows)]
pub use native::collect;

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn collect(_: &mut Connection) -> Result<usize> { anyhow::bail!("Windows only") }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_sysmon_dns_xml() {
        let xml = r#"<Event xmlns="http://schemas.microsoft.com/win/2004/08/events/event"><System><Provider Name="Microsoft-Windows-Sysmon"/><EventID>22</EventID><EventRecordID>152</EventRecordID><TimeCreated SystemTime="2026-11-04T19:05:01.123Z"/></System><EventData><Data Name="ProcessGuid">{abcdef}</Data><Data Name="ProcessId">1234</Data><Data Name="QueryName">www.example.com</Data><Data Name="QueryResults">10.0.0.1</Data></EventData></Event>"#;
        let event = parse_xml(xml).unwrap();
        assert_eq!(event.event_id,22);
        assert_eq!(event.record_id,152);
        assert_eq!(field(&event, "QueryName"),Some("www.example.com"));
        assert_eq!(event.timestamp_utc,"2026-11-04T19:05:01.123Z");
    }
    #[test]
    fn rolls_up_non_browser_connections_and_ignores_browser_subresources() {
        let mut db=Connection::open_in_memory().unwrap();
        crate::db::init(&db).unwrap();
        let event=|record_id,event_id,image:&str| SysmonEvent {
            record_id,event_id,timestamp_utc:"2026-11-04T22:10:30.000Z".into(),
            data:HashMap::from([
                ("Image".to_string(),image.to_string()),
                ("DestinationIp".to_string(),"93.184.215.14".to_string()),
                ("DestinationPort".to_string(),"443".to_string()),
                ("Protocol".to_string(),"tcp".to_string()),
                ("QueryName".to_string(),"example.com".to_string()),
            ]),
        };
        let batch=vec![
            event(10,3,r"C:\\Apps\\Spotify.exe"),
            event(11,3,r"C:\\Apps\\Spotify.exe"),
            event(12,22,r"C:\\Apps\\Spotify.exe"),
            event(13,3,r"C:\\Program Files\\Mozilla Firefox\\firefox.exe"),
            event(14,22,r"C:\\Program Files\\Mozilla Firefox\\firefox.exe"),
        ];
        insert_events(&mut db,&batch).unwrap();
        insert_events(&mut db,&batch).unwrap();
        let (buckets,connections):(i64,i64)=db.query_row(
            "SELECT count(*),sum(connection_count) FROM app_connection_hours",[],
            |r|Ok((r.get(0)?,r.get(1)?)),
        ).unwrap();
        assert_eq!((buckets,connections),(1,2));
        let (dns_buckets,queries):(i64,i64)=db.query_row(
            "SELECT count(*),sum(query_count) FROM app_domain_minutes",[],
            |r|Ok((r.get(0)?,r.get(1)?)),
        ).unwrap();
        assert_eq!((dns_buckets,queries),(1,1));
        let raw:i64=db.query_row("SELECT count(*) FROM network_events",[],|r|r.get(0)).unwrap();
        assert_eq!(raw,0,"new collector should not append raw connection noise");
    }

    #[test]
    fn stored_events_are_deduplicated() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        let e = SysmonEvent { record_id: 2, event_id: 22,
            timestamp_utc:"2026-11-04T19:05:01.123Z".into(),
            data: HashMap::from([("QueryName".into(), "example.com".into())]) };
        insert_events(&mut conn, &[e.clone()]).unwrap();
        insert_events(&mut conn, &[e]).unwrap();
        let count:i64 = conn.query_row("SELECT COUNT(*) FROM app_domain_minutes",[],|r|r.get(0)).unwrap();
        assert_eq!(count,1);
        crate::db::set_meta(&conn, "sysmon_epoch", "1").unwrap();
        crate::db::set_meta(&conn, "sysmon_last_record_id", "0").unwrap();
        let repeated = SysmonEvent { record_id: 2, event_id: 22,
            timestamp_utc:"2026-11-05T19:05:01.123Z".into(),
            data: HashMap::from([("QueryName".into(), "new.example.com".into())]) };
        insert_events(&mut conn, &[repeated]).unwrap();
        let count:i64 = conn.query_row("SELECT COUNT(*) FROM app_domain_minutes",[],|r|r.get(0)).unwrap();
        assert_eq!(count,2, "record IDs from distinct log generations must be preserved");
    }
}
