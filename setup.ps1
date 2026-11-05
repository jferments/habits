# One-command setup for the Rust collector (run from a normal PowerShell session).
# Builds/tests, configures Sysmon if needed, installs a per-user startup task,
# starts habagent.exe, and prints a diagnostic status. Idempotent on reruns.
[CmdletBinding()]
param(
    [switch]$SkipSysmon,
    [switch]$SkipTests
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

try {
    if ($env:OS -ne 'Windows_NT') { throw 'Setup requires Windows.' }
    $Root = $PSScriptRoot
    $Cargo = Get-Command 'cargo.exe' -ErrorAction SilentlyContinue
    if ($null -eq $Cargo) {
        throw 'Cargo was not found. Install the Rust MSVC toolchain, then reopen PowerShell.'
    }

    Push-Location -LiteralPath $Root
    try {
        if (-not $SkipTests) {
            Write-Host '==> Testing the Rust collector'
            & $Cargo.Source test --all-targets
            if ($LASTEXITCODE -ne 0) { throw 'cargo test failed.' }
        }
        Write-Host '==> Building habits.exe and habagent.exe'
        & $Cargo.Source build --release
        if ($LASTEXITCODE -ne 0) { throw 'cargo build --release failed.' }

        if (-not $SkipSysmon) {
            $Services = @(Get-Service -Name 'Sysmon', 'Sysmon64' -ErrorAction SilentlyContinue)
            $Running = @($Services | Where-Object { $_.Status -eq 'Running' })
            if ($Running.Count -gt 0) {
                Write-Host '==> Sysmon is already running; preserving its existing configuration.'
                Write-Warning 'If its configuration omits Event IDs 1, 3, 5, or 22, those event categories will not be captured.'
            } else {
                Write-Host '==> Preparing Sysmon (administrator approval may be requested)'
                $Helper = Join-Path $Root 'installer\setup-sysmon.ps1'
                $PowerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
                $Identity = [Security.Principal.WindowsIdentity]::GetCurrent()
                $Principal = New-Object Security.Principal.WindowsPrincipal($Identity)
                if ($Principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
                    & $PowerShell -NoProfile -ExecutionPolicy Bypass -File $Helper
                    $Code = $LASTEXITCODE
                } else {
                    # Elevate only the short Sysmon helper. Compile and per-user
                    # installation stay in the original, unelevated account.
                    $ElevationArgs = '-NoProfile -ExecutionPolicy Bypass -File "' + $Helper + '"'
                    $Process = Start-Process -FilePath $PowerShell -ArgumentList $ElevationArgs -Verb RunAs -Wait -PassThru
                    $Code = $Process.ExitCode
                }
                if ($Code -ne 0) {
                    throw "Sysmon setup failed (exit $Code). Read its error above and rerun setup.ps1 after resolving it."
                }
            }
        } else {
            Write-Warning 'Sysmon setup skipped. DNS, connection, and process capture require Sysmon.'
        }

        Write-Host '==> Installing the current-user startup task'
        & (Join-Path $Root 'installer\install.ps1') -SkipSysmonReminder

        Write-Host '==> Starting the windowless background collector'
        Start-ScheduledTask -TaskName 'HabitsCollector' -ErrorAction Stop
        # A successful Start-ScheduledTask call only queues the request. Check for
        # an actual running task AND a freshly updated SQLite heartbeat.
        $Cli = Join-Path $Root 'target\release\habits.exe'
        $Database = Join-Path $env:LOCALAPPDATA 'Habits\activity.sqlite'
        $Healthy = $false
        $Status = @()
        $Task = $null
        for ($Attempt = 1; $Attempt -le 20; $Attempt++) {
            Start-Sleep -Seconds 1
            $Task = Get-ScheduledTask -TaskName 'HabitsCollector' -ErrorAction Stop
            if ($Task.State -ne 'Running' -or -not (Test-Path -LiteralPath $Database)) { continue }
            $Status = @(& $Cli status 2>&1)
            if ($LASTEXITCODE -eq 0 -and (($Status | Out-String) -match 'Last heartbeat: .*\(recent\)')) {
                $Healthy = $true
                break
            }
        }
        if (-not $Healthy) {
            Write-Warning "HabitsCollector failed startup verification. Task state: $($Task.State); database exists: $(Test-Path -LiteralPath $Database)."
            & (Join-Path $Root 'installer\doctor.ps1')
            throw 'Background collector did not start and produce a fresh database heartbeat. Setup is NOT complete.'
        }
        Write-Host '==> Collector status'
        $Status | ForEach-Object { Write-Host $_ }
        if (($Status | Out-String) -match 'Sysmon status: error:') {
            Write-Warning 'Sysmon event ingestion is reporting an error. Foreground tracking should still work.'
        }

        if (-not $SkipSysmon) {
            try {
                $null = Get-WinEvent -ListLog 'Microsoft-Windows-Sysmon/Operational' -ErrorAction Stop
            } catch {
                Write-Warning ("Unable to inspect Sysmon Operational channel: " + $_.Exception.Message)
                Write-Warning 'This is a separate Sysmon access/availability issue; the collector itself is now running.'
            }
        }
        Write-Host ''
        Write-Host 'Setup complete. Habits starts quietly at each user logon.'
        Write-Host 'No Add/Remove Programs entry or tray icon is created.'
        Write-Host 'Open a NEW terminal to use: habits status'
        Write-Host 'To export later: habits export --from YYYY-MM-DD --to YYYY-MM-DD'
    } finally {
        Pop-Location
    }
} catch {
    Write-Host ("Habits setup failed: " + $_.Exception.Message) -ForegroundColor Red
    Write-Host 'For read-only diagnostics, run: powershell.exe -NoProfile -File .\installer\doctor.ps1'
    exit 1
}
