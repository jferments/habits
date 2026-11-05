use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct Product {
    key: String,
    name: String,
    version: Option<String>,
    publisher: Option<String>,
    source: String,
}

pub fn scan_and_update(conn: &mut Connection) -> Result<usize> {
    #[cfg(not(windows))]
    { let _ = conn; anyhow::bail!("Windows inventory only") }
    #[cfg(windows)]
    {
        let mut products = registry_products()?;
        let msix_ok = match msix_products() {
            Ok(items) => { products.extend(items); true },
            Err(e) => {
                crate::db::record_error(conn, "inventory", &format!("MSIX scan unavailable: {e:#}"))?;
                false
            }
        };
        let timestamp = crate::util::timestamp();
        let existing: HashMap<String,Product> = {
            let mut q = conn.prepare("SELECT product_key,display_name,display_version,publisher,source FROM software_current")?;
            let rows = q.query_map([], |r| Ok(Product {
                key: r.get(0)?, name: r.get(1)?, version: r.get(2)?,
                publisher: r.get(3)?, source: r.get(4)?,
            }))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?.into_iter()
                .map(|p| (p.key.clone(),p)).collect()
        };
        let current: HashMap<String, Product> = products.into_iter().map(|p| (p.key.clone(),p)).collect();
        let tx = conn.transaction()?;
        let mut changes = 0;
        for (key, product) in &current {
            let new_type = match existing.get(key) {
                None => Some(if existing.is_empty() { "baseline" } else { "installed" }),
                Some(old) if old.version != product.version => Some("updated"),
                _ => None,
            };
            if let Some(kind) = new_type {
                let old_version = existing.get(key).and_then(|p| p.version.as_deref());
                tx.execute(r#"INSERT INTO software_changes
                    (detected_utc,product_key,display_name,event_type,old_version,new_version,source)
                    VALUES(?1,?2,?3,?4,?5,?6,?7)"#,
                    params![timestamp, key, product.name, kind, old_version,
                        product.version, product.source])?;
                changes += 1;
            }
            tx.execute(r#"INSERT INTO software_current
                (product_key,display_name,display_version,publisher,source,first_seen_utc,last_seen_utc)
                VALUES(?1,?2,?3,?4,?5,?6,?6)
                ON CONFLICT(product_key) DO UPDATE SET
                 display_name=excluded.display_name, display_version=excluded.display_version,
                 publisher=excluded.publisher, source=excluded.source, last_seen_utc=excluded.last_seen_utc"#,
                 params![key, product.name, product.version, product.publisher, product.source, timestamp])?;
        }
        for (key, old) in &existing {
            // Do not misreport removals when MSIX scanning failed.
            if !current.contains_key(key) && (msix_ok || old.source != "msix") {
                tx.execute(r#"INSERT INTO software_changes
                    (detected_utc,product_key,display_name,event_type,old_version,new_version,source)
                    VALUES(?1,?2,?3,'removed',?4,NULL,?5)"#,
                    params![timestamp, key, old.name, old.version, old.source])?;
                tx.execute("DELETE FROM software_current WHERE product_key=?1", [key])?;
                changes += 1;
            }
        }
        crate::db::set_meta(&tx, "last_inventory_utc", &timestamp)?;
        tx.commit()?;
        Ok(changes)
    }
}

#[cfg(windows)]
fn registry_products() -> Result<Vec<Product>> {
    use winreg::{enums::*, RegKey};
    let mut result = Vec::new();
    let uninstall = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    for (root_label, root, view_label, view) in [
        ("HKLM", RegKey::predef(HKEY_LOCAL_MACHINE), "64", KEY_WOW64_64KEY),
        ("HKLM", RegKey::predef(HKEY_LOCAL_MACHINE), "32", KEY_WOW64_32KEY),
        ("HKCU", RegKey::predef(HKEY_CURRENT_USER), "64", KEY_WOW64_64KEY),
    ] {
        let reg = root.open_subkey_with_flags(uninstall, KEY_READ | view)?;
        for subkey_name in reg.enum_keys().flatten() {
            let Ok(key) = reg.open_subkey_with_flags(&subkey_name, KEY_READ | view) else { continue };
            if key.get_value::<u32,_>("SystemComponent").unwrap_or_default() == 1 { continue; }
            let Ok(name) = key.get_value::<String, _>("DisplayName") else { continue };
            if name.trim().is_empty() { continue; }
            result.push(Product {
                key: format!("registry/{root_label}/{view_label}/{subkey_name}"),
                name,
                version: key.get_value("DisplayVersion").ok(),
                publisher: key.get_value("Publisher").ok(),
                source: "registry".to_string(),
            });
        }
    }
    Ok(result)
}

#[cfg(windows)]
fn msix_products() -> Result<Vec<Product>> {
    use anyhow::Context;
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8; Get-AppxPackage | Select-Object Name,@{N='Version';E={$_.Version.ToString()}},PackageFamilyName,Publisher | ConvertTo-Json -Compress";
    let output = Command::new("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output().context("launching AppX inventory")?;
    anyhow::ensure!(output.status.success(), "Get-AppxPackage failed: {}",
        String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8(output.stdout)?.trim_start_matches('\u{feff}').trim().to_owned();
    if text.is_empty() { return Ok(Vec::new()); }
    let items: serde_json::Value = serde_json::from_str(&text)?;
    let objects: Vec<&serde_json::Value> = match &items {
        serde_json::Value::Array(x) => x.iter().collect(),
        serde_json::Value::Object(_) => vec![&items],
        _ => vec![],
    };
    let mut out = Vec::new();
    for item in objects {
        let Some(name) = item.get("Name").and_then(|v| v.as_str()) else { continue };
        let family = item.get("PackageFamilyName").and_then(|v| v.as_str()).unwrap_or(name);
        out.push(Product {
            key: format!("msix/{family}"),
            name: name.to_string(),
            version: item.get("Version").and_then(|v| v.as_str()).map(str::to_owned),
            publisher: item.get("Publisher").and_then(|v| v.as_str()).map(str::to_owned),
            source: "msix".to_string(),
        });
    }
    Ok(out)
}
