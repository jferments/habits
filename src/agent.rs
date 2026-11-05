use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::OpenOptions;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
pub fn run() -> Result<()> {
    let dir = crate::util::data_dir()?;
    std::fs::create_dir_all(&dir)?;
    let lock_path = dir.join("collector.lock");
    let lock = OpenOptions::new().read(true).write(true).create(true).open(lock_path)?;
    lock.try_lock_exclusive().context("another Habits collector is already running")?;

    let mut conn = crate::db::open(&crate::util::database_path()?)?;
    crate::db::set_meta(&conn, "collector_version", env!("CARGO_PKG_VERSION"))?;
    crate::db::set_meta(&conn, "last_heartbeat_utc", &crate::util::timestamp())?;
    let mut foreground = crate::win_activity::ForegroundMonitor::new(120);
    let mut last_event_poll = Instant::now() - Duration::from_secs(6);
    let mut last_history_poll = Instant::now() - Duration::from_secs(65);
    let mut last_heartbeat = Instant::now() - Duration::from_secs(11);
    let mut last_inventory = Instant::now() - Duration::from_secs(6 * 3600 + 1);
    let mut last_sysmon_error: Option<String> = None;
    // Do not rerun a full software inventory on every login if it ran recently.
    if let Some(value) = crate::db::get_meta(&conn, "last_inventory_utc")? {
        if let Ok(saved) = chrono::DateTime::parse_from_rfc3339(&value) {
            let elapsed = (chrono::Utc::now() - saved.with_timezone(&chrono::Utc)).num_seconds();
            if (0..(6 * 3600)).contains(&elapsed) {
                last_inventory = Instant::now() - Duration::from_secs(elapsed as u64);
            }
        }
    }
    loop {
        // An interactive agent must run in the logged-in user's Windows session.
        if let Err(error) = foreground.sample(&conn) {
            crate::db::record_error(&conn, "foreground", &format!("{error:#}"))?;
        }

        if last_event_poll.elapsed() >= Duration::from_secs(5) {
            match crate::sysmon::collect(&mut conn) {
                Ok(_) => {
                    last_sysmon_error = None;
                    crate::db::set_meta(&conn, "sysmon_status", "ok")?;
                }
                Err(err) => {
                    let msg = format!("{err:#}");
                    if last_sysmon_error.as_deref() != Some(msg.as_str()) {
                        crate::db::record_error(&conn, "sysmon", &msg)?;
                        last_sysmon_error = Some(msg.clone());
                    }
                    crate::db::set_meta(&conn, "sysmon_status", &format!("error: {msg}"))?;
                }
            }
            last_event_poll = Instant::now();
        }

        // Browser history is OPTIONAL. SQLite read locks / schema changes /
        // missing browser profiles cannot terminate the base agent.
        if last_history_poll.elapsed() >= Duration::from_secs(60) {
            if let Err(error) = crate::browser_history::poll(&mut conn, 30) {
                let _ = crate::db::set_meta(
                    &conn, "browser_history_status", &format!("error: {error:#}")
                );
            }
            last_history_poll = Instant::now();
        }

        if last_inventory.elapsed() >= Duration::from_secs(6 * 3600) {
            if let Err(e) = crate::inventory::scan_and_update(&mut conn) {
                crate::db::record_error(&conn, "inventory", &format!("{e:#}"))?;
            }
            last_inventory = Instant::now();
        }

        if last_heartbeat.elapsed() >= Duration::from_secs(10) {
            crate::db::set_meta(&conn, "last_heartbeat_utc", &crate::util::timestamp())?;
            last_heartbeat = Instant::now();
        }
        thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn run() -> Result<()> { anyhow::bail!("Windows only") }
