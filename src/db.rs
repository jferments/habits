use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 1;

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    init(&conn)?;
    Ok(conn)
}

pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS browser_seen_events (
            event_id TEXT PRIMARY KEY,
            created_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS browser_visits (
            event_id TEXT PRIMARY KEY,
            browser TEXT NOT NULL,
            host TEXT NOT NULL,
            visited_utc TEXT NOT NULL,
            tab_id INTEGER,
            window_id INTEGER
        );
        CREATE INDEX IF NOT EXISTS ix_browser_visits_time ON browser_visits(visited_utc);
        CREATE INDEX IF NOT EXISTS ix_browser_visits_host_time ON browser_visits(host,visited_utc);
        CREATE TABLE IF NOT EXISTS browser_attention_intervals (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            browser TEXT NOT NULL,
            host TEXT NOT NULL,
            tab_id INTEGER,
            window_id INTEGER,
            started_ms INTEGER NOT NULL,
            ended_ms INTEGER NOT NULL,
            CHECK (ended_ms >= started_ms)
        );
        CREATE INDEX IF NOT EXISTS ix_browser_attention_time ON browser_attention_intervals(started_ms);
        CREATE TABLE IF NOT EXISTS browser_focus_state (
            browser TEXT PRIMARY KEY,
            host TEXT NOT NULL,
            interval_id INTEGER NOT NULL REFERENCES browser_attention_intervals(id),
            last_ms INTEGER NOT NULL,
            tab_id INTEGER,
            window_id INTEGER
        );
        -- Non-browser Sysmon metadata is coalesced instead of stored per packet/
        -- connection. DNS timestamps do NOT establish a network connection.
        CREATE TABLE IF NOT EXISTS app_domain_minutes (
            app TEXT NOT NULL,
            host TEXT NOT NULL,
            minute_utc TEXT NOT NULL,
            first_utc TEXT NOT NULL,
            last_utc TEXT NOT NULL,
            query_count INTEGER NOT NULL,
            PRIMARY KEY(app,host,minute_utc)
        );
        CREATE INDEX IF NOT EXISTS ix_app_domain_minutes_time ON app_domain_minutes(minute_utc);
        CREATE TABLE IF NOT EXISTS app_connection_hours (
            app TEXT NOT NULL,
            destination_ip TEXT NOT NULL,
            destination_port INTEGER NOT NULL,
            protocol TEXT NOT NULL,
            hour_utc TEXT NOT NULL,
            first_utc TEXT NOT NULL,
            last_utc TEXT NOT NULL,
            connection_count INTEGER NOT NULL,
            PRIMARY KEY(app,destination_ip,destination_port,protocol,hour_utc)
        );
        CREATE INDEX IF NOT EXISTS ix_app_connection_hours_time ON app_connection_hours(hour_utc);
        CREATE TABLE IF NOT EXISTS sysmon_rollup_seen (
            log_epoch INTEGER NOT NULL,
            record_id INTEGER NOT NULL,
            PRIMARY KEY(log_epoch,record_id)
        );
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY, value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS process_events (
            log_epoch INTEGER NOT NULL DEFAULT 0,
            record_id INTEGER NOT NULL,
            timestamp_utc TEXT NOT NULL,
            event_id INTEGER NOT NULL,
            process_guid TEXT,
            pid INTEGER,
            image TEXT,
            parent_guid TEXT,
            PRIMARY KEY(log_epoch,record_id)
        );
        CREATE INDEX IF NOT EXISTS ix_process_events_time ON process_events(timestamp_utc);
        CREATE INDEX IF NOT EXISTS ix_process_events_guid ON process_events(process_guid);

        CREATE TABLE IF NOT EXISTS dns_events (
            log_epoch INTEGER NOT NULL DEFAULT 0,
            record_id INTEGER NOT NULL,
            timestamp_utc TEXT NOT NULL,
            process_guid TEXT,
            pid INTEGER,
            image TEXT,
            query_name TEXT NOT NULL,
            query_status TEXT,
            query_results TEXT,
            PRIMARY KEY(log_epoch,record_id)
        );
        CREATE INDEX IF NOT EXISTS ix_dns_events_time ON dns_events(timestamp_utc);
        CREATE INDEX IF NOT EXISTS ix_dns_events_name ON dns_events(query_name);
        CREATE INDEX IF NOT EXISTS ix_dns_events_guid ON dns_events(process_guid);

        CREATE TABLE IF NOT EXISTS network_events (
            log_epoch INTEGER NOT NULL DEFAULT 0,
            record_id INTEGER NOT NULL,
            timestamp_utc TEXT NOT NULL,
            process_guid TEXT,
            pid INTEGER,
            image TEXT,
            protocol TEXT,
            source_ip TEXT,
            source_port INTEGER,
            destination_ip TEXT,
            destination_port INTEGER,
            initiated INTEGER,
            PRIMARY KEY(log_epoch,record_id)
        );
        CREATE INDEX IF NOT EXISTS ix_network_events_time ON network_events(timestamp_utc);
        CREATE INDEX IF NOT EXISTS ix_network_events_guid ON network_events(process_guid);

        CREATE TABLE IF NOT EXISTS activity_intervals (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            started_utc TEXT NOT NULL,
            ended_utc TEXT NOT NULL,
            pid INTEGER,
            image TEXT,
            idle INTEGER NOT NULL,
            CHECK (ended_utc > started_utc)
        );
        CREATE INDEX IF NOT EXISTS ix_activity_interval_start ON activity_intervals(started_utc);
        CREATE INDEX IF NOT EXISTS ix_activity_interval_end ON activity_intervals(ended_utc);

        CREATE TABLE IF NOT EXISTS software_current (
            product_key TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            display_version TEXT,
            publisher TEXT,
            source TEXT NOT NULL,
            first_seen_utc TEXT NOT NULL,
            last_seen_utc TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS software_changes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            detected_utc TEXT NOT NULL,
            product_key TEXT NOT NULL,
            display_name TEXT NOT NULL,
            event_type TEXT NOT NULL,
            old_version TEXT,
            new_version TEXT,
            source TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS ix_software_changes_time ON software_changes(detected_utc);

        CREATE TABLE IF NOT EXISTS collector_errors (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp_utc TEXT NOT NULL,
            component TEXT NOT NULL,
            message TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS ix_collector_errors_time ON collector_errors(timestamp_utc);
    "#)?;
    let found: Option<String> = conn.query_row(
        "SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0)
    ).optional()?;
    match found {
        Some(v) if v != SCHEMA_VERSION.to_string() => {
            anyhow::bail!("unsupported database schema {v}, expected {SCHEMA_VERSION}")
        }
        None => set_meta(conn, "schema_version", &SCHEMA_VERSION.to_string())?,
        _ => {}
    }
    Ok(())
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional()?)
}

pub fn record_error(conn: &Connection, component: &str, message: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO collector_errors(timestamp_utc,component,message) VALUES(?1,?2,?3)",
        params![crate::util::timestamp(), component, message],
    )?;
    Ok(())
}

pub fn show_status(path: &Path) -> Result<()> {
    if !path.exists() {
        println!("No database yet: {}", path.display());
        return Ok(());
    }
    // Status is intentionally read-only: it cannot accidentally create a new DB.
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    println!("Habits Collector — {}", path.display());
    let hb: Option<String> = get_meta(&conn, "last_heartbeat_utc")?;
    match &hb {
        Some(ts) => {
            let alive = chrono::DateTime::parse_from_rfc3339(ts)
                .map(|x| (chrono::Utc::now() - x.with_timezone(&chrono::Utc)).num_seconds() < 40)
                .unwrap_or(false);
            println!("Last heartbeat: {ts}  ({})", if alive { "recent" } else { "stale / stopped" });
        }
        None => println!("Last heartbeat: none"),
    }
    for (name, table) in [
        ("Process events", "process_events"),
        ("DNS queries", "dns_events"),
        ("Connections", "network_events"),
        ("Activity intervals", "activity_intervals"),
        ("Software changes", "software_changes"),
        ("Collector errors", "collector_errors"),
    ] {
        let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
        println!("{name:20} {count}");
    }
    let browser_visits: i64 = conn.query_row("SELECT count(*) FROM browser_visits", [], |r|r.get(0))?;
    let browser_intervals: i64 = conn.query_row("SELECT count(*) FROM browser_attention_intervals", [], |r|r.get(0))?;
    println!("Browser visits        {browser_visits}");
    println!("Browser attention     {browser_intervals}");
    let app_domains: i64 = conn.query_row("SELECT count(*) FROM app_domain_minutes", [], |r|r.get(0))?;
    let app_connections: i64 = conn.query_row("SELECT count(*) FROM app_connection_hours", [], |r|r.get(0))?;
    println!("App DNS minutes       {app_domains}");
    println!("App connection hours  {app_connections}");
    println!("Sysmon status: {}", get_meta(&conn, "sysmon_status")?.as_deref().unwrap_or("unknown"));
    println!("Sysmon checkpoint: {}", get_meta(&conn, "sysmon_last_record_id")?.as_deref().unwrap_or("uninitialized"));
    Ok(())
}
