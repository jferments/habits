# Stops only the Habits scheduled task and optionally removes its binaries.
# Database and Sysmon are deliberately left untouched. Browser host registry
# entries and copied extension files are removed with -RemoveBinaries.
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
    Remove-Item -LiteralPath (Join-Path $Folder 'habbridge.exe') -ErrorAction SilentlyContinue
    foreach ($RegistryKey in @(
        'HKCU\\Software\\Mozilla\\NativeMessagingHosts\\org.jferments.habits',
        'HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\org.jferments.habits'
    )) {
        & reg.exe delete $RegistryKey /f 2>$null | Out-Null
    }
    Remove-Item -LiteralPath (Join-Path $env:LOCALAPPDATA 'Habits\\native-host') -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath (Join-Path $env:LOCALAPPDATA 'Habits\\extensions') -Recurse -Force -ErrorAction SilentlyContinue
    # Clean up the old pre-rename binary, if present.
    Remove-Item -LiteralPath (Join-Path $Folder 'habits-agent.exe') -ErrorAction SilentlyContinue
}
Write-Host 'Habits task removed. Data retained in %LOCALAPPDATA%\Habits.'
Write-Host 'Sysmon configuration/service was not modified.'
