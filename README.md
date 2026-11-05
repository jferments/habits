# Habits — Windows collector and browser domain journal (Rust prototype)

NOTE: THIS IS A THROWAWAY PROJECT THAT IS JUST BEING USED AS A ONE OFF SCRIPT FOR PERSONAL USE ... 
USE AT YOUR OWN RISK <3

Local-only Windows activity collector and one-line date-range ZIP exporter.

**Status: initial Rust collector with Windows compilation/unit-test coverage.**
Windows compilation and the original installer have been confirmed; extended
end-to-end collection (especially Sysmon permissions and reboot behavior) still
needs verification on a real Windows machine. This is not a kernel service,
packet sniffer, security tool, or complete web-browsing history recorder.

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
  `habits.exe` with `run`, `status`, `sites`, `app-domains`, `inventory`, and `export` commands.
- Firefox and Edge MV3 domain journal add-ons, connected to a small
  `habbridge.exe` native messaging process (no local HTTP server).
- Timestamped browser top-level hostname visits and bounded active-tab
  intervals, stored in SQLite without full URLs, queries, or titles.
- Non-browser DNS lookups coalesced by **app/domain/minute** and network
  connection records by **app/destination IP/port/hour** instead of
  saving all raw network events.
- Date-range ZIP containing filtered SQLite and `manifest.json` with SHA-256.
- Per-user scheduled task and minimal Sysmon config template.

**Browser domain tracking requires separately loading the Firefox or Edge
extension.** The `setup.ps1` script installs the Rust bridge and registers
native messaging, but does not silently install an extension into the browser.
See [browser installation and verification](extensions/README.md). Browsers
still require user installation consent; persistent Firefox installation
requires signing (not yet performed). The add-ons do not capture URL paths.

## Quick start — one setup command

From a **normal, non-elevated PowerShell** session in your already-cloned
`habits` directory, update the source and run setup:

```powershell
git pull --ff-only
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\setup.ps1
```

The setup command is safe to rerun after updates. It:

1. Runs Rust tests and compiles release `habits.exe`, `habagent.exe`, and
   the transient browser native messaging host `habbridge.exe`.
2. Checks for Sysmon. When absent, it requests administrator approval **only
   for Sysmon setup**, prefers built-in Windows Sysmon when available, or
   downloads Sysinternals Sysmon directly from Microsoft and validates its
   digital signature before installing. The approved Sysmon installation
   accepts Microsoft's Sysmon EULA.
3. Installs/updates the per-user `HabitsCollector` Scheduled Task (no Installed
   Apps entry, shortcut, tray icon, or custom always-on Windows service).
4. Registers per-user browser native messaging and copies unpacked add-on
   source to stable folders. **You still need to load the add-ons in Firefox
   and Edge** and register Edge's shown extension ID.
5. Starts the collector immediately and verifies the task is running and
   SQLite heartbeat is fresh. If not, it runs the read-only diagnostic script
   and **fails setup** rather than falsely claiming success.

Sysmon Event IDs **1, 3, 5, and 22** are enabled for **new** installations
using `installer/sysmon-config.xml`. These supply process, connection, and
DNS telemetry. The setup **does not replace an existing Sysmon configuration**
or reinstall an existing Sysmon service. If a pre-existing configuration
omits those events, you must explicitly merge the rules yourself; it may
otherwise yield no network/DNS records.

**Why administrator approval?** Sysmon installs a Microsoft-signed Windows
service/driver. The Rust collector still runs **under your regular login**.
Some Windows builds require a reboot to enable the optional Sysmon feature;
if prompted, reboot and rerun the same setup command.

`-ExecutionPolicy Bypass` applies to this one PowerShell process only; the
installer does not change your machine's execution-policy settings. Review
the repository scripts before running them. The Sysmon Operational log itself
may include command lines and other metadata beyond what Habits retains;
it is not a content-free upstream log.

Use `habits status` from a **new terminal** after setup. Data resides in
`%LOCALAPPDATA%\Habits\activity.sqlite`. If Sysmon reports an error, inspect
`%LOCALAPPDATA%\Habits\agent-errors.log` and the Sysmon Operational log
permissions. The installer does not silently change channel security ACLs.

Optional setup switches for troubleshooting:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\setup.ps1 -SkipSysmon
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\setup.ps1 -SkipTests
```

`-SkipSysmon` is for intentional partial setup only: DNS/network/process
collection will not work without Sysmon.

## Diagnosing a collector that is Queued or has no database

A successful Rust build or an error-free `Start-ScheduledTask` invocation does
**not** prove that the collector is running. The database at
`%LOCALAPPDATA%\Habits\activity.sqlite` is created **before** Sysmon ingestion,
so `habits status` showing "No database yet" means the agent has not reached
its SQLite initialization. A queued task is one possible cause.

The installer now permits starting on battery power and uses the Windows logon
identity plus an explicit working directory. Setup checks both the task state
and a **recent collector heartbeat**, and exits nonzero if either is missing.

To run a read-only diagnostic from the repository directory:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\installer\doctor.ps1
```

This prints the task state, last-run code, executable path, power restrictions,
the `habagent.exe` process, any fatal error log, and the exact Sysmon-channel
access error. It does not modify the Event Log ACL or other security settings.

If no agent process is running, you can test it in the foreground to see startup
errors immediately:

```powershell
.\target\release\habits.exe run
```

Leave it running briefly, check `habits status` in another terminal, then press
Ctrl+C. Stop an already-running Habits task before a foreground test to avoid
the exclusive collector lock.

**Sysmon warnings are separate from task startup.** A non-elevated
`Get-WinEvent -ListLog` failure may mean access denied, missing channel, or
another error. First inspect the exact diagnostic and confirm Sysmon's service.
Do not broaden the whole Event Log Readers group or replace existing channel
security descriptors automatically. Sysmon's upstream log may contain
sensitive command-line metadata.

## Sysmon channel permission blocked (Access is denied / error 5)

If `habits status` shows a **recent heartbeat**, and the diagnostic says
`Sysmon / Running`, but `Sysmon status` reports **EvtQuery: Access is denied**
then the Rust collector is running correctly and **only Sysmon ingestion is
blocked**. The Windows collector runs with your ordinary interactive account;
running it as SYSTEM or permanently elevated is **not** the fix.

Windows allows read access to be granted on one Event Log channel without
granting read access to every Event Log. The narrowest suitable permission is
an **allow, READ (0x1)** rule for the intended user's Windows SID on
`Microsoft-Windows-Sysmon/Operational`. This access can expose additional
metadata contained in Sysmon's own upstream log, including process command
lines, even though Habits stores fewer fields.

**Administrator procedure** (do not run blindly on a shared or managed PC):

1. In your ordinary PowerShell, run `whoami /user` to identify your account SID.
2. In a separate administrator PowerShell, inspect the existing channel
   descriptor with `wevtutil gl Microsoft-Windows-Sysmon/Operational`.
3. Back up the original `channelAccess` SDDL string, then use Windows'
   `wevtutil sl <channel> /ca:<updated-SDDL>` to add one allow-read (0x1)
   entry for **that SID only**, preserving every existing ACE and any owner,
   group or audit sections. Do not replace the descriptor with a canned one.
   Microsoft documents these event-log access rights and the `wevtutil`
   `/ca` option.
4. From your **normal non-admin** PowerShell, confirm that
   `Get-WinEvent -ListLog 'Microsoft-Windows-Sysmon/Operational'` no longer
   fails, then run `habits status` after the collector's next poll.
5. If access remains denied, inspect for an existing deny ACE or policies
   managing the log; **do not weaken other log permissions**.

The one-command setup intentionally **does not alter Sysmon security ACLs**.
An existing installation is never reconfigured silently. For troubleshooting,
run `powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\installer\doctor.ps1`.

The broad alternative of adding the account to the local
`Event Log Readers` group may grant access to multiple channels and is
not recommended for this per-user monitoring app.

## Requirements

- Windows 11 x64 (successful Rust compilation; end-to-end runtime validation ongoing).
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

This should produce `target\release\habits.exe`,
`target\release\habagent.exe`, and `target\release\habbridge.exe`.

## Manual Sysmon setup (optional; the quick start handles fresh installations)

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

## Manual agent install (optional; the quick start already runs this)

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

If execution policy blocks local scripts, use the quick-start `powershell.exe -ExecutionPolicy Bypass -File .\\setup.ps1` form rather than changing execution policy system-wide.

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
habits sites --limit 50
habits app-domains --limit 50
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

- `browser_visits`: **hostname + exact visit timestamp**, Firefox/Edge.
- `browser_attention_intervals`: bounded active-tab time by hostname.
- `app_domain_minutes`: non-browser DNS queries, by executable/host/minute.
- `app_connection_hours`: non-browser endpoint counts by executable/IP/port/hour.
- `process_events`: timestamped process events.
- `dns_events`, `network_events`: legacy raw events from older collectors
  are retained but new entries are **not** appended; no automatic data deletion.
- `activity_intervals`: foreground PID/executable and idle estimates.
- `software_current`, `software_changes`: software inventory and change log.
- `meta`, `collector_errors`: source checkpoints, heartbeat, and diagnostics.

The export format is versioned. It can be imported by Linux SQLite tools even
before we develop a GUI analyzer.

## Data safety

The application does **not** capture keystrokes, full URLs or page paths,
browser titles, content, clipboard, screenshots, HTTP bodies, cookies, or passwords.
Private/incognito tabs are excluded by design. WebExtensions log visit
hostnames and approximate active-tab time after the user installs them. No server is opened
and no network packets are modified. The setup changes only a scheduled task
and, **if explicitly configured by you**, Sysmon settings.
