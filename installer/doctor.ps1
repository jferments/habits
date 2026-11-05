# Read-only diagnostics for the Habits per-user startup task and Sysmon.
# Does not install, grant privileges, change settings, or delete data.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'

Write-Host ''
Write-Host '===== Habits diagnostic report ====='
Write-Host ("Current identity: " + [Security.Principal.WindowsIdentity]::GetCurrent().Name)
Write-Host ("LOCALAPPDATA: " + $env:LOCALAPPDATA)
$Task = Get-ScheduledTask -TaskName 'HabitsCollector' -ErrorAction SilentlyContinue

if ($null -eq $Task) {
    Write-Warning 'Scheduled task HabitsCollector is not registered.'
} else {
    Write-Host ("Task state: " + $Task.State)
    Write-Host ("Task principal: " + $Task.Principal.UserId)
    Write-Host ("Task logon type: " + $Task.Principal.LogonType)
    Write-Host ("Run level: " + $Task.Principal.RunLevel)
    Write-Host ("Run on batteries: " + (-not $Task.Settings.DisallowStartIfOnBatteries))
    Write-Host ("Stop on batteries: " + $Task.Settings.StopIfGoingOnBatteries)
    Write-Host ("Run only if idle: " + $Task.Settings.RunOnlyIfIdle)
    Write-Host ("Run only if network available: " + $Task.Settings.RunOnlyIfNetworkAvailable)
    Write-Host ("Start when available: " + $Task.Settings.StartWhenAvailable)

    foreach ($Action in @($Task.Actions)) {
        Write-Host ("Task executable: " + $Action.Execute)
        Write-Host ("Executable exists: " + (Test-Path -LiteralPath $Action.Execute))
        Write-Host ("Working directory: " + $Action.WorkingDirectory)
    }
    try {
        $Info = Get-ScheduledTaskInfo -TaskName 'HabitsCollector' -ErrorAction Stop
        Write-Host ("Last run: " + $Info.LastRunTime)
        $ResultHex = "0x{0:X8}" -f ([uint32]($Info.LastTaskResult -band 0xFFFFFFFFL))
        Write-Host ("LastTaskResult: " + $Info.LastTaskResult + " (" + $ResultHex + ")")
        Write-Host ("Missed runs: " + $Info.NumberOfMissedRuns)
    } catch { Write-Warning ("Task history unavailable: " + $_.Exception.Message) }
}

try {
    $Processes = @(Get-CimInstance Win32_Process -Filter "name = 'habagent.exe'" -ErrorAction Stop)
    if ($Processes.Count -eq 0) {
        Write-Warning 'No habagent.exe process is running.'
    } else {
        foreach ($Process in $Processes) {
            Write-Host ("Running habagent.exe PID: " + $Process.ProcessId + "  " + $Process.ExecutablePath)
        }
    }
} catch {
    Write-Warning ("Cannot query agent processes: " + $_.Exception.Message)
}

$DataDir = Join-Path $env:LOCALAPPDATA 'Habits'
$DbPath = Join-Path $DataDir 'activity.sqlite'
Write-Host ("Database exists: " + (Test-Path -LiteralPath $DbPath))
Write-Host ("Expected database: " + $DbPath)
$ErrorLog = Join-Path $DataDir 'agent-errors.log'
if (Test-Path -LiteralPath $ErrorLog) {
    Write-Host 'Recent agent fatal errors:'
    Get-Content -LiteralPath $ErrorLog -Tail 10
} else {
    Write-Host 'No agent-errors.log found.'
}

$Services = @(Get-Service -Name 'Sysmon', 'Sysmon64' -ErrorAction SilentlyContinue)
if ($Services.Count -eq 0) {
    Write-Warning 'No Sysmon/Sysmon64 service was detected.'
} else {
    foreach ($Service in $Services) { Write-Host ("Sysmon service: " + $Service.Name + " / " + $Service.Status) }
}
try {
    $Log = Get-WinEvent -ListLog 'Microsoft-Windows-Sysmon/Operational' -ErrorAction Stop
    Write-Host ("Sysmon channel accessible: True; Enabled: " + $Log.IsEnabled)
} catch {
    Write-Warning ("Cannot read Sysmon channel: " + $_.Exception.Message)
    Write-Host 'This error may indicate missing read permission or an unavailable channel. No permissions have been changed.'
}

try {
    $Recent = @(Get-WinEvent -FilterHashtable @{
        LogName = 'Microsoft-Windows-TaskScheduler/Operational'
        StartTime = (Get-Date).AddHours(-3)
    } -MaxEvents 80 -ErrorAction Stop | Where-Object { $_.Message -like '*HabitsCollector*' } | Select-Object -First 8)
    if ($Recent.Count -gt 0) {
        Write-Host 'Recent HabitsCollector Task Scheduler events:'
        foreach ($Event in $Recent) {
            Write-Host ("  " + $Event.TimeCreated.ToString('s') + " Event " + $Event.Id + ": " +
                (($Event.Message -replace '\r?\n', ' ') -replace '\s+', ' ').Substring(0,
                [Math]::Min(240, (($Event.Message -replace '\r?\n', ' ') -replace '\s+', ' ').Length)))
        }
    } else {
        Write-Host 'No recent matching Task Scheduler events (history may be disabled).'
    }
} catch {
    Write-Host ("Task Scheduler history unavailable: " + $_.Exception.Message)
}
Write-Host '===== End diagnostics ====='
Write-Host ''
