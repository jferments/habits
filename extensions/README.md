# Habits Domain Journal — Firefox and Microsoft Edge

This is a local-only WebExtension and a Rust native messaging bridge. It records
top-level **hostnames** and their visit timestamps, plus bounded time during
which a hostname was in the focused browser tab. It never sends data to a
remote server.

## Install / update the native bridge (Windows)

In a **normal, non-administrator PowerShell** from the Habits checkout:

```powershell
git pull --ff-only
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\setup.ps1
```

This builds `habbridge.exe` and registers a per-user Firefox native messaging
host. Browser extension source is copied to these stable folders:

- `%LOCALAPPDATA%\Habits\extensions\firefox`
- `%LOCALAPPDATA%\Habits\extensions\edge`

The Windows registry host keys are limited to the current user's account.
No separate Python/Node runtime, cloud service, HTTP listener, or admin
permission is needed for the browser bridge.

## Firefox — development/testing

1. Open `about:debugging#/runtime/this-firefox` in Firefox.
2. Choose **Load Temporary Add-on** and select
   `%LOCALAPPDATA%\Habits\extensions\firefox\manifest.json`.
3. Visit a couple of websites. Run `habits sites --limit 15`.

**Important:** A temporary Firefox add-on is removed after Firefox restarts.
For **persistent installation on stable Firefox**, the extension needs Mozilla
signing (for example, submitting an unlisted build to Mozilla Add-ons).
Signing has **not** been performed as part of this prototype; it requires
a developer account and distribution steps. Do not assume the temporary
extension will survive a restart.

## Microsoft Edge — load unpacked

1. In Edge, open `edge://extensions` and turn on **Developer mode**.
2. Click **Load unpacked** and select
   `%LOCALAPPDATA%\Habits\extensions\edge` (the folder).
3. Copy the extension's **ID**, a 32-character string shown on that page.
4. In your normal PowerShell, from the repo, run:

   ```powershell
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\setup.ps1 -SkipTests -EdgeExtensionId YOUR_32_CHARACTER_ID
   ```

5. On `edge://extensions`, click **Reload** for Habits Domain Journal.
   Visit websites and check `habits sites --limit 15`.

The ID is different for different extension builds/installation paths.
The host refuses unknown IDs via an explicit `allowed_origins` list.
Subsequent installs remember the registered ID unless it changes. If the
extension is installed through the Edge store in the future, its ID might
change; re-register that new ID.

**Why can't setup silently install both extensions?** Browsers require user
consent/developer loading, and stable Firefox requires signed packages. The
native host can be installed automatically, but the browser-side add-on
cannot be assumed to be installed/authorized just because `setup.ps1` succeeded.

## What gets captured

- `browser_visits`: each HTTP(S) top-level navigation (including SPA
  history updates). `browser`, `host`, timestamp, tab/window IDs and an
  event ID. Repeat navigations to the same host get distinct visit records,
  except immediate duplicate navigation notifications.
- `browser_attention_intervals`: active-tab hostname, start and bounded
  end timestamps. The add-on sends focus changes and approximately
  30-second heartbeats. On abrupt exit or sleep, an interval never counts
  more than 45 seconds past its last heartbeat.
- `app_domain_minutes`: **non-browser DNS lookup observations**, not
  confirmed connections, bucketed by process executable/host/minute.
- `app_connection_hours`: **non-browser network connection counts**,
  bucketed by executable/IP/port/protocol/hour. An IP cannot safely be
  equated to a domain when CDNs, DNS caches and encrypted DNS are involved.

Tabs loaded in the background still produce navigation timestamps; time
is counted only when a browser window and that tab are active. The
current idle cutoff is 90 seconds, so reading without input or watching
long videos can be undercounted. This is an estimate of attention, not a
measurement of cognition. Visits start **when the extension is loaded**:
we do not scrape the prior full browser-history database.

Private/incognito tabs are deliberately **not collected**. Non-HTTP(S)
pages and internal browser pages are excluded. URL paths, parameters,
fragments, titles, cookies, form data, and page bodies are **never stored**.
The extension buffers up to 20,000 unsent events locally when the bridge
is unavailable. Overflow is indicated with a red `!` badge; it can lose
the oldest events, so this is not guaranteed lossless if offline for
an extended period. Never silently claim historical coverage.

## Verify

In a normal PowerShell:

```powershell
habits status
habits sites --limit 30
habits app-domains --limit 30
```

If visits don't appear, inspect the browser extension error console. In Edge
verify `-EdgeExtensionId` and reload the extension. Firefox's native messaging
manifest is registered automatically, but the temporary add-on must be loaded
in `about:debugging`.

The separate `habbridge.exe` process lives only while a browser is
connected to it; `habagent.exe` remains the quiet startup collector.
