# Run from PowerShell (normal user) after cargo build --release.
# Optional: -ConfigureSysmon -SysmonExe C:\Tools\Sysmon64.exe  (requires elevation)
[CmdletBinding()]
param(
    [switch]$ConfigureSysmon,
    [string]$SysmonExe = 'Sysmon64.exe',
    [switch]$SkipSysmonReminder,
    [string]$EdgeExtensionId = ''
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Release = Join-Path $Root 'target\release'
$SourceCli = Join-Path $Release 'habits.exe'
$SourceAgent = Join-Path $Release 'habagent.exe'
$SourceBridge = Join-Path $Release 'habbridge.exe'
if (!(Test-Path $SourceCli) -or !(Test-Path $SourceAgent)) {
    throw 'Build first: cargo build --release'
}
# Intentionally install as an unlisted, per-user background tool: no MSI/MSIX,
# no Add/Remove Programs (Uninstall registry) entry, and no Start-menu shortcut.
# Auto-start is registered only via the visible HabitsCollector scheduled task.
$Install = Join-Path $env:LOCALAPPDATA 'Habits\bin'
New-Item -ItemType Directory -Force -Path $Install | Out-Null
# A running agent can lock the executable during an upgrade.
$CurrentTask = Get-ScheduledTask -TaskName 'HabitsCollector' -ErrorAction SilentlyContinue
if ($CurrentTask) {
    Stop-ScheduledTask -TaskName 'HabitsCollector' -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 600
}
# A stopped task may take a moment to release the executable during upgrades.
# Retry the copy rather than failing spuriously with a file-in-use error.
foreach ($Source in @($SourceCli, $SourceAgent)) {
    $Destination = Join-Path $Install (Split-Path -Leaf $Source)
    $Copied = $false
    for ($Attempt = 0; $Attempt -lt 20; $Attempt++) {
        try {
            Copy-Item -LiteralPath $Source -Destination $Destination -Force -ErrorAction Stop
            $Copied = $true
            break
        } catch [System.IO.IOException] {
            Start-Sleep -Milliseconds 500
        }
    }
    if (-not $Copied) { throw "Could not update $Destination. Stop HabitsCollector and try again." }
}
$AgentPath = Join-Path $Install 'habagent.exe'
# Run on the current user's next logon. The agent has Windows GUI subsystem,
# intentionally no console window; scheduler process does not require elevation.
$Identity = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$Action = New-ScheduledTaskAction -Execute $AgentPath -WorkingDirectory $Install
$Trigger = New-ScheduledTaskTrigger -AtLogOn -User $Identity
$Principal = New-ScheduledTaskPrincipal -UserId $Identity -LogonType Interactive -RunLevel Limited
# Laptop battery conditions otherwise cause a task to remain Queued without
# launching the collector. Keep it interactive (foreground tracking needs the
# user session), without a time limit or idle/network requirements.
$Settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds 0) `
    -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -StartWhenAvailable
Register-ScheduledTask -TaskName 'HabitsCollector' -Action $Action -Trigger $Trigger -Principal $Principal -Settings $Settings -Force | Out-Null
# Optional browser integration. Never fail the base collector installation or
# its Task Scheduler registration because of missing/broken browser components.
# habbridge.exe runs only on demand under Firefox/Edge, in a separate process.
$BrowserOptionalReady = $false
try {
    if (-not (Test-Path -LiteralPath $SourceBridge)) {
        throw 'habbridge.exe was not built; the core collector remains installed.'
    }
    $BridgeTarget = Join-Path $Install 'habbridge.exe'
    $Copied = $false
    for ($Attempt = 0; $Attempt -lt 20; $Attempt++) {
        try {
            Copy-Item -LiteralPath $SourceBridge -Destination $BridgeTarget -Force -ErrorAction Stop
            $Copied = $true
            break
        } catch [System.IO.IOException] {
            # Browser might currently have the bridge open. Never stop habagent.
            Start-Sleep -Milliseconds 500
        }
    }
    if (-not $Copied) { throw 'Could not update the browser bridge (probably in use).' }
    $ExtensionSrc = Join-Path $Root 'extensions'
    $ExtensionDest = Join-Path $env:LOCALAPPDATA 'Habits\extensions'
    foreach ($Browser in @('firefox','edge')) {
        $From = Join-Path $ExtensionSrc $Browser
        $To = Join-Path $ExtensionDest $Browser
        if (!(Test-Path -LiteralPath (Join-Path $From 'manifest.json')) -or
            !(Test-Path -LiteralPath (Join-Path $From 'background.js'))) {
            throw "Missing optional browser extension files: $From"
        }
        New-Item -ItemType Directory -Path $To -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $From 'manifest.json') -Destination $To -Force
        Copy-Item -LiteralPath (Join-Path $From 'background.js') -Destination $To -Force
    }
    & (Join-Path $PSScriptRoot 'register-browser-hosts.ps1') -EdgeExtensionId $EdgeExtensionId
    $BrowserOptionalReady = $true
    Write-Host ("Optional browser add-ons copied to " + $ExtensionDest)
} catch {
    Write-Warning ("Optional browser extension setup incomplete: " + $_.Exception.Message)
    Write-Warning 'The core HabitsCollector startup task is installed and is unaffected.'
}

# Add CLI folder to user's PATH without changing the machine PATH.
$OldPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$Entries = @($OldPath -split ';' | Where-Object { $_ })
if ($Entries -notcontains $Install) {
    [Environment]::SetEnvironmentVariable('Path', (($Entries + $Install) -join ';'), 'User')
}
if ($ConfigureSysmon) {
    $Config = Join-Path $PSScriptRoot 'sysmon-config.xml'
    Write-Warning 'Applying this config REPLACES the Sysmon configuration already on your machine.'
    & $SysmonExe -c $Config
    if ($LASTEXITCODE -ne 0) {
        Write-Warning 'Sysmon -c failed: if Sysmon is not installed, install it using Sysmon64.exe -accepteula -i sysmon-config.xml from elevated PowerShell.'
    }
}
Write-Host "Installed CLI and agent into $Install"
Write-Host 'Task: HabitsCollector (runs quietly at the next interactive logon)'
Write-Host 'No Installed Apps / Add or Remove Programs entry is registered.'
if ($BrowserOptionalReady) {
    Write-Host 'Firefox: load temporary add-on from %LOCALAPPDATA%\Habits\extensions\firefox\manifest.json'
    Write-Host 'Edge: load unpacked folder %LOCALAPPDATA%\Habits\extensions\edge and register its ID.'
}
Write-Host 'Run now: Start-ScheduledTask -TaskName HabitsCollector'
Write-Host 'Open a new terminal, then: habits status'
if (!$ConfigureSysmon -and !$SkipSysmonReminder) {
    Write-Warning 'This installer does not configure Sysmon. Use .\\setup.ps1 for automated Sysmon setup and startup.'
}
