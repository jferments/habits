use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

pub fn export_zip(input: &Path, output: &Path, from: NaiveDate, to: NaiveDate) -> Result<()> {
    anyhow::ensure!(input.is_file(), "database not found: {}", input.display());
    anyhow::ensure!(!output.exists(), "output already exists: {}", output.display());
    let (start_utc, end_exclusive_utc) = crate::util::utc_boundaries(from, to)?;
    let dir = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let scratch = tempfile::tempdir()?;
    let filtered_db_path = scratch.path().join("activity.sqlite");
    {
        let dst = Connection::open(&filtered_db_path)?;
        crate::db::init(&dst)?;
        dst.execute_batch("PRAGMA journal_mode=DELETE;")?;
    }

    let src = Connection::open_with_flags(input, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    src.busy_timeout(std::time::Duration::from_secs(10))?;
    src.execute("ATTACH DATABASE ?1 AS report", params![filtered_db_path.to_string_lossy().to_string()])?;
    // WAL reader snapshot: all SELECT statements use the same source snapshot.
    src.execute_batch("BEGIN DEFERRED TRANSACTION;")?;
    let _: i64 = src.query_row("SELECT COUNT(*) FROM main.sqlite_master", [], |r| r.get(0))?;
    let copied: Result<()> = (|| {
        src.execute(r#"INSERT INTO report.dns_events
            SELECT * FROM main.dns_events WHERE timestamp_utc >= ?1 AND timestamp_utc < ?2"#,
            params![start_utc, end_exclusive_utc])?;
        src.execute(r#"INSERT INTO report.network_events
            SELECT * FROM main.network_events WHERE timestamp_utc >= ?1 AND timestamp_utc < ?2"#,
            params![start_utc, end_exclusive_utc])?;
        // Include creation metadata for processes whose network events occur in range.
        src.execute(r#"INSERT INTO report.process_events
            SELECT * FROM main.process_events WHERE
            (timestamp_utc >= ?1 AND timestamp_utc < ?2)
            OR (event_id=1 AND process_guid IN (
                SELECT process_guid FROM report.dns_events WHERE process_guid IS NOT NULL
                UNION SELECT process_guid FROM report.network_events WHERE process_guid IS NOT NULL
            ))"#, params![start_utc, end_exclusive_utc])?;
        // Clip intervals crossing the selected boundaries, rather than dropping them.
        src.execute(r#"INSERT INTO report.activity_intervals(id,started_utc,ended_utc,pid,image,idle)
            SELECT id,
              CASE WHEN started_utc < ?1 THEN ?1 ELSE started_utc END,
              CASE WHEN ended_utc > ?2 THEN ?2 ELSE ended_utc END,
              pid,image,idle
            FROM main.activity_intervals
            WHERE started_utc < ?2 AND ended_utc > ?1"#,
            params![start_utc, end_exclusive_utc])?;
        // Preserve baseline and all earlier changes to reconstruct software state.
        src.execute(r#"INSERT INTO report.software_changes
            SELECT * FROM main.software_changes WHERE detected_utc < ?1"#,
            params![end_exclusive_utc])?;
        src.execute(r#"INSERT INTO report.collector_errors
            SELECT * FROM main.collector_errors WHERE timestamp_utc >= ?1 AND timestamp_utc < ?2"#,
            params![start_utc, end_exclusive_utc])?;
        src.execute(r#"INSERT INTO report.meta(key,value)
            SELECT key,value FROM main.meta WHERE key='schema_version' OR key='collector_version'
            ON CONFLICT(key) DO UPDATE SET value=excluded.value"#, [])?;
        Ok(())
    })();
    match copied {
        Ok(()) => src.execute_batch("COMMIT;")?,
        Err(e) => { let _ = src.execute_batch("ROLLBACK;"); return Err(e); }
    }
    src.execute_batch("DETACH DATABASE report;")?;
    drop(src);

    let mut dbfile = File::open(&filtered_db_path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = dbfile.read(&mut buffer)?;
        if count == 0 { break; }
        hasher.update(&buffer[..count]);
    }
    let db_sha256 = format!("{:x}", hasher.finalize());
    let report = Connection::open_with_flags(&filtered_db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let count = |name: &str| -> Result<i64> {
        // All values below are fixed schema table names, not external input.
        Ok(report.query_row(&format!("SELECT COUNT(*) FROM {name}"),[],|r|r.get(0))?)
    };
    let counts = json!({
        "process_events": count("process_events")?,
        "dns_events": count("dns_events")?,
        "network_events": count("network_events")?,
        "activity_intervals": count("activity_intervals")?,
        "software_changes": count("software_changes")?,
        "collector_errors": count("collector_errors")?
    });
    drop(report);

    let manifest = json!({
        "format": "habits-report",
        "schema_version": crate::db::SCHEMA_VERSION,
        "collector_version": env!("CARGO_PKG_VERSION"),
        "exported_at_utc": Utc::now().to_rfc3339(),
        "from_date_local_inclusive": from.to_string(),
        "to_date_local_inclusive": to.to_string(),
        "start_utc_inclusive": start_utc,
        "end_utc_exclusive": end_exclusive_utc,
        "data_file": "activity.sqlite",
        "data_file_sha256": db_sha256,
        "rows": counts,
        "notes": [
            "Software changes include earlier baseline events to reconstruct the inventory.",
            "Foreground time and idle time are estimates, not direct measures of attention.",
            "Sysmon DNS and network events do not by themselves establish website visits."
        ]
    });

    let tempzip = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut zip = ZipWriter::new(tempzip.reopen()?);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("manifest.json", options)?;
        zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
        zip.start_file("activity.sqlite", options)?;
        let mut source = File::open(&filtered_db_path)?;
        std::io::copy(&mut source, &mut zip)?;
        zip.finish()?;
    }
    tempzip.persist(output).with_context(|| format!("writing {}", output.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn range_is_local_day_based_and_ordered() {
        let from = chrono::NaiveDate::from_ymd_opt(2026,11,1).unwrap();
        let to = chrono::NaiveDate::from_ymd_opt(2026,11,2).unwrap();
        let (a,b) = crate::util::utc_boundaries(from,to).unwrap();
        assert!(a < b);
        assert!(crate::util::utc_boundaries(to,from).is_err());
    }
}
