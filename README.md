# Habits (Windows collector) — Rust prototype v0.2

Local-only Windows activity collector and one-line date-range ZIP exporter.

**Status: source prototype, not yet Windows-build-verified.** The authoring environment
had no Rust compiler or Windows runtime; compile and run the included tests on Windows
before relying on it for permanent monitoring. The SQLite schema and Sysmon XML were
validated separately. This is not a kernel service, packet sniffer, security tool,
parental control, or complete web-browsing history recorder.

## What is implemented

- Native modern Windows Event Log (`wevtapi.dll`) collection of Sysmon Event IDs
  1 (process creation), 5 (exit), 3 (network connection), and 22 (DNS).
- Crash-resumable Event Record ID checkpoint, deduplicated writes, and detection
  of a reset/cleared Sysmon event log.
- Foreground application PID + executable path with timestamps and idle-state
  estimation (1-second polling, 120-second idle threshold). No titles/keys/pages.
- Per-user and machine uninstall-registry inventory (64/32-bit registry views),
  plus the current user's MSIX/AppX package inventory every 6 hours.
- Baseline, installed, updated, and removed software-change records.
- Local SQLite database in WAL mode, in `%LOCALAPPDATA%\Habits\activity.sqlite`.
- Quiet separate `habagent.exe` with no console window; a console CLI
  `habits.exe` with `run`, `status`, `inventory`, and `export` commands.
- Date-range ZIP containing filtered SQLite and `manifest.json` with SHA-256.
- Per-user scheduled task and minimal Sysmon config template.

**The Windows app does not yet know which browser tab is active.** DNS/connection
records alone cannot reliably establish which websites you visited or how long
you spent on them. Browser extension support will be an additional Windows-side
feature, before the later Linux analyzer.

## Requirements

- Windows 11 x64 (intended; the native collector has not been runtime-tested yet).
- A current Rust stable toolchain with Cargo and MSVC C++ build tools for native
  dependencies (SQLite bundled with `rusqlite`); see https://rustup.rs/ .
- PowerShell 5.1+ for MSIX software inventory and optional scheduled-task installation.
- Sysmon, configured to emit relevant event IDs, for DNS/network/process telemetry.
  Windows 11 built-in Sysmon or Microsoft Sysinternals Sysmon may be used;
  use only one installed implementation.

## Build

In a Windows PowerShell terminal, from the unpacked project directory:

```powershell
cargo test
cargo build --release
```

This should produce `target\release\habits.exe` and
`target\release\habagent.exe`.

## Sysmon setup — explicit administrator operation

**Do not blindly replace an existing Sysmon configuration.** If you already use
Sysmon for another purpose, merge these event types into your existing config
instead. The sample config in `installer\sysmon-config.xml` logs *all* matching
process, DNS, and connection events, which can be high volume.

If Sysmon is not already installed, use an **elevated PowerShell session** with
an official copy of Sysmon, from Microsoft's Sysinternals download page:

```powershell
.\Sysmon64.exe -accepteula -i .\installer\sysmon-config.xml
```

If you already have Sysmon installed and explicitly want to REPLACE its config:

```powershell
.\Sysmon64.exe -c .\installer\sysmon-config.xml
```

On newer Windows 11 installations with built-in Sysmon, follow Microsoft's
built-in Sysmon enable/install instructions instead of simultaneously installing
Sysinternals Sysmon. The included config targets schema 4.90; check compatibility
with your installed Sysmon version (`sysmon -s`) before applying.

**Important privacy caveat:** The Sysmon Windows Event Log may contain command
lines and other sensitive system data in its own event records, even though the
Habits database deliberately does not persist those fields. Filtering what our
collector stores does *not* eliminate that information from the upstream Sysmon log.

Ordinary users may lack permission to read the Sysmon Operational event channel.
If `habits status` reports `EvtQuery failed`, use Event Viewer to confirm the
channel is present and review its read permissions, or run a controlled test
with elevated permission. Avoid running the interactive agent under SYSTEM;
that would prevent monitoring your foreground desktop session.

## Install background agent

In a **non-elevated** PowerShell session from the project directory:

```powershell
.\installer\install.ps1
Start-ScheduledTask -TaskName HabitsCollector
```

Then in a **new** terminal (so the user `PATH` change takes effect):

```powershell
habits status
```

The scheduled task launches `habagent.exe`, which is a windowless Windows
GUI-subsystem executable. It is not an interactive program and does not display
notifications. It launches at each logon. Sysmon itself runs as a system service,
so its logging can continue when this per-user agent is not running, subject to
the Event Log channel's retention/rollover.

### Installation visibility (intentional)

Habits is deliberately a **per-user background startup task**, not an entry in
**Settings > Apps > Installed apps** (or **Add or Remove Programs**). The
installer does not create a Windows `Uninstall` registry entry, MSI/MSIX
package, Start menu shortcut, or system-tray application. The only regular
startup registration it creates is the `HabitsCollector` scheduled task,
which is visible and manageable in Task Scheduler. The installer also adds
the CLI folder to the *user* PATH so `habits` works in a terminal.

A scheduled task is preferable to a Session 0 Windows service for foreground
window and idle tracking: `habagent.exe` needs to run in your interactive
user session. It starts at **user logon**, not before you sign in. Sysmon is
a separate Microsoft component and runs as a system service.

To stop auto-start and remove the collector binaries explicitly:

```powershell
.\installer\uninstall.ps1 -RemoveBinaries
```

The uninstaller leaves the activity database and Sysmon untouched. The
scheduled task is not hidden: it should be easy to discover and disable
whenever you choose. No popups or Add/Remove Programs entry are needed.

If your PowerShell execution policy blocks the installer, review the script
before allowing it to run. Don't disable execution policy system-wide.

## Export a date range

```powershell
habits export --from 2026-11-01 --to 2026-11-30
```

By default the ZIP is written to `%LOCALAPPDATA%\Habits\exports`. To choose
another location:

```powershell
habits export --from 2026-11-01 --to 2026-11-30 --output D:\Reports\november.zip
```

Both calendar dates are inclusive in the Windows machine's local time zone. The
ZIP contains `manifest.json` and `activity.sqlite`. All UTC timestamps in the
SQLite file have fixed `YYYY-MM-DDTHH:MM:SS.mmmZ` format for ordering.

The exporter clips foreground intervals to the selected date bounds, includes
process-creation metadata for DNS/network processes seen during the range, and
includes software baseline/history prior to `--to` so the analyzer can
reconstruct inventory state. Exports do not overwrite existing ZIPs.

**No remote upload occurs**; copy the ZIP manually to your Linux computer.

## CLI commands

```powershell
habits status
habits inventory
habits export --from YYYY-MM-DD --to YYYY-MM-DD [--output PATH]
# troubleshooting (console visible):
habits run
```

`habits run` and `habagent.exe` cannot run simultaneously: they use an
exclusive `collector.lock` file. `habits inventory` is available for tests but
should not normally be invoked while the background collector is running.

Stop/uninstall just the Habits task:

```powershell
.\installer\uninstall.ps1
```

That script does not delete your data and does not change or uninstall Sysmon.

## Limits and next milestones

1. **User-attention estimates:** A foreground window is not proof of attention,
   and passive video viewing can register as idle. Lock/sleep events are not yet
   tracked separately. 1-second polling may miss very short switches, and up to
   60 seconds of the most recent ongoing activity interval can be lost on abrupt
   termination.
2. **Network coverage:** Sysmon's DNS queries are not all DNS traffic (e.g.,
   browser DoH), and Event ID 3 is connection metadata, **not byte counts**,
   packets, or connection lifetime. No HTTP bodies or URL paths are collected.
   Sysmon's network-connection logging must be explicitly enabled.
3. **Inventory:** Registry and current-user MSIX snapshots detect changes on
   periodic scans, so `detected_utc` is not necessarily the actual change time.
   Portable software and other users' MSIX installations are not comprehensively
   enumerated.
4. **Sysmon log retention:** If a high-volume Sysmon channel overwrites events
   before the collector catches up, data is lost. Review Event Viewer log size
   and retention. The collector detects cleared/restarted record IDs but does
   not yet prove completeness under all log-rotation conditions.
5. **Browser use:** Later add a small opt-in Firefox/Chromium native-messaging
   extension recording active hostnames/tab intervals only (no page bodies,
   search strings, cookies, private mode).
6. **Stability:** Before using long-term, run on Windows and review CPU/RAM,
   Event Log permissions, sleep/lock behavior, software scan errors, database
   corruption recovery, and task reboot behavior.
7. **Security:** Local SQLite and ZIP are not encrypted by default. Restrict
   file access to your Windows account; use disk encryption and protect ZIPs
   when moving them to another machine.

## Storage overview

- `process_events`, `dns_events`, `network_events`: timestamped Sysmon events.
- `activity_intervals`: foreground PID/executable and idle estimates.
- `software_current`, `software_changes`: software inventory and change log.
- `meta`, `collector_errors`: source checkpoints, heartbeat, and diagnostics.

The export format is versioned. It can be imported by Linux SQLite tools even
before we develop a GUI analyzer.

## Data safety

The application does **not** capture keystrokes, browser titles, content,
clipboard, screenshots, HTTP bodies, cookies, or passwords. No server is opened
and no network packets are modified. The setup changes only a scheduled task
and, **if explicitly configured by you**, Sysmon settings.
