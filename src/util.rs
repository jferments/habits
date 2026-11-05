use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate, SecondsFormat, TimeZone, Utc};
use std::path::PathBuf;

pub fn data_dir() -> Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .context("LOCALAPPDATA is not defined (the collector expects Windows)")?;
    Ok(PathBuf::from(base).join("Habits"))
}

pub fn database_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("activity.sqlite"))
}

// Inclusive local calendar dates converted to half-open UTC boundaries.
// chrono::Local handles Windows time-zone transitions; ambiguous midnight is rare,
// but checked explicitly so we never silently misattribute data.
pub fn utc_boundaries(start: NaiveDate, end: NaiveDate) -> Result<(String, String)> {
    anyhow::ensure!(end >= start, "--to must be on or after --from");
    let end_exclusive = end.succ_opt().context("invalid end date")?;
    let left = local_midnight(start)?;
    let right = local_midnight(end_exclusive)?;
    Ok((left.to_rfc3339_opts(SecondsFormat::Millis, true), right.to_rfc3339_opts(SecondsFormat::Millis, true)))
}

fn local_midnight(date: NaiveDate) -> Result<DateTime<Utc>> {
    let midnight = date.and_hms_opt(0, 0, 0).context("invalid date")?;
    Local.from_local_datetime(&midnight).earliest()
        .context("local midnight unavailable on this date")
        .map(|x| x.with_timezone(&Utc))
}

pub fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
