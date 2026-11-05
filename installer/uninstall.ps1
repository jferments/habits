# Stops only the Habits scheduled task and optionally removes its binaries.
# Database and Sysmon are deliberately left untouched.
[CmdletBinding()]
param([switch]$RemoveBinaries)
$ErrorActionPreference = 'Stop'
$Task = Get-ScheduledTask -TaskName 'HabitsCollector' -ErrorAction SilentlyContinue
if ($Task) {
    Stop-ScheduledTask -TaskName 'HabitsCollector' -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName 'HabitsCollector' -Confirm:$false
}
if ($RemoveBinaries) {
    $Folder = Join-Path $env:LOCALAPPDATA 'Habits\bin'
    Remove-Item -LiteralPath (Join-Path $Folder 'habits.exe') -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath (Join-Path $Folder 'habagent.exe') -ErrorAction SilentlyContinue
    # Clean up the old pre-rename binary, if present.
    Remove-Item -LiteralPath (Join-Path $Folder 'habits-agent.exe') -ErrorAction SilentlyContinue
}
Write-Host 'Habits task removed. Data retained in %LOCALAPPDATA%\Habits.'
Write-Host 'Sysmon configuration/service was not modified.'
