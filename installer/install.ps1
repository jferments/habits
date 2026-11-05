# Run from PowerShell (normal user) after cargo build --release.
# Optional: -ConfigureSysmon -SysmonExe C:\Tools\Sysmon64.exe  (requires elevation)
[CmdletBinding()]
param(
    [switch]$ConfigureSysmon,
    [string]$SysmonExe = 'Sysmon64.exe'
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Release = Join-Path $Root 'target\release'
$SourceCli = Join-Path $Release 'habits.exe'
$SourceAgent = Join-Path $Release 'habagent.exe'
if (!(Test-Path $SourceCli) -or !(Test-Path $SourceAgent)) {
    throw 'Build first: cargo build --release'
}
$Install = Join-Path $env:LOCALAPPDATA 'Habits\bin'
New-Item -ItemType Directory -Force -Path $Install | Out-Null
# A running agent can lock the executable during an upgrade.
$CurrentTask = Get-ScheduledTask -TaskName 'HabitsCollector' -ErrorAction SilentlyContinue
if ($CurrentTask) {
    Stop-ScheduledTask -TaskName 'HabitsCollector' -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 600
}
Copy-Item $SourceCli $Install -Force
Copy-Item $SourceAgent $Install -Force
$AgentPath = Join-Path $Install 'habagent.exe'
# Run on the current user's next logon. The agent has Windows GUI subsystem,
# intentionally no console window; scheduler process does not require elevation.
$Action = New-ScheduledTaskAction -Execute $AgentPath
$Trigger = New-ScheduledTaskTrigger -AtLogOn -User "$env:USERDOMAIN\$env:USERNAME"
$Principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited
$Settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds 0) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName 'HabitsCollector' -Action $Action -Trigger $Trigger -Principal $Principal -Settings $Settings -Force | Out-Null
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
Write-Host 'Run now: Start-ScheduledTask -TaskName HabitsCollector'
Write-Host 'Open a new terminal, then: habits status'
if (!$ConfigureSysmon) {
    Write-Warning 'Sysmon was NOT installed/configured. See README.md; DNS/connection telemetry requires Sysmon configured separately.'
}
