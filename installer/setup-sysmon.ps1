# Requires administrator permissions; launched by setup.ps1 only when needed.
# Installs Microsoft Sysmon without overwriting a pre-existing installation/config.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$TemporaryFolder = $null

try {
    $Principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $Principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Sysmon setup requires an elevated PowerShell process.'
    }

    $Config = Join-Path $PSScriptRoot 'sysmon-config.xml'
    if (-not (Test-Path -LiteralPath $Config)) { throw "Missing Sysmon config: $Config" }

    $Existing = @(Get-Service -Name 'Sysmon', 'Sysmon64' -ErrorAction SilentlyContinue)
    if ($Existing.Count -gt 0) {
        Write-Host 'Existing Sysmon detected: its configuration will NOT be replaced.'
        foreach ($Service in $Existing) {
            if ($Service.Status -ne 'Running') {
                Start-Service -Name $Service.Name -ErrorAction Stop
            }
        }
    } else {
        # Built-in Sysmon is preferred when the OS exposes the optional feature.
        # Never enable it alongside standalone Sysmon (checked above).
        $Feature = $null
        if (Get-Command 'Get-WindowsOptionalFeature' -ErrorAction SilentlyContinue) {
            try {
                $Feature = Get-WindowsOptionalFeature -Online -FeatureName Sysmon -ErrorAction Stop
            } catch {
                # Older Windows builds do not provide the feature.
                $Feature = $null
            }
        }

        if ($null -ne $Feature) {
            Write-Host 'Windows built-in Sysmon feature is available.'
            if ($Feature.State -eq 'Disabled') {
                $Result = Enable-WindowsOptionalFeature -Online -FeatureName Sysmon -NoRestart -ErrorAction Stop
                if ($Result.RestartNeeded) {
                    throw 'Windows enabled the Sysmon feature but requires a restart. Reboot and rerun setup.ps1.'
                }
            } elseif ($Feature.State -ne 'Enabled') {
                throw "Sysmon optional feature is in state '$($Feature.State)'. Restart Windows if pending and rerun setup.ps1."
            }

            $Executable = Join-Path $env:SystemRoot 'System32\sysmon.exe'
            if (-not (Test-Path -LiteralPath $Executable)) {
                throw 'Windows built-in sysmon.exe is not available yet. Restart Windows, then rerun setup.ps1.'
            }
            Write-Host 'Installing built-in Sysmon using the Habits event configuration...'
            & $Executable -accepteula -i $Config
            if ($LASTEXITCODE -ne 0) { throw "Built-in Sysmon installation failed (exit $LASTEXITCODE)." }
        } else {
            # The standalone download is only used when the built-in option is absent.
            $TemporaryFolder = Join-Path $env:TEMP ("Habits-Sysmon-" + [Guid]::NewGuid().ToString('N'))
            New-Item -ItemType Directory -Force -Path $TemporaryFolder | Out-Null
            $Archive = Join-Path $TemporaryFolder 'Sysmon.zip'
            $DownloadUrl = 'https://download.sysinternals.com/files/Sysmon.zip'
            Write-Host 'Built-in Sysmon not available. Downloading standalone Sysmon from Microsoft...'
            $ProgressPreference = 'SilentlyContinue'
            Invoke-WebRequest -Uri $DownloadUrl -OutFile $Archive -UseBasicParsing -ErrorAction Stop
            Expand-Archive -LiteralPath $Archive -DestinationPath $TemporaryFolder -ErrorAction Stop
            $Executable = Join-Path $TemporaryFolder 'Sysmon64.exe'
            if (-not (Test-Path -LiteralPath $Executable)) { throw 'Microsoft Sysmon archive did not contain Sysmon64.exe.' }

            $Signature = Get-AuthenticodeSignature -FilePath $Executable
            if ($Signature.Status -ne 'Valid' -or
                $null -eq $Signature.SignerCertificate -or
                $Signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
                throw 'Downloaded Sysmon64.exe did not have a valid Microsoft Authenticode signature.'
            }

            Write-Host 'Verified Microsoft signature; installing standalone Sysmon...'
            & $Executable -accepteula -i $Config
            if ($LASTEXITCODE -ne 0) { throw "Standalone Sysmon installation failed (exit $LASTEXITCODE)." }
        }

        # The initial Habits filter includes DNS, network, and process metadata.
        # A moderate log size reduces loss from Event Log rollover during brief gaps.
        # Do not change retention settings on existing Sysmon installations.
        & wevtutil.exe sl 'Microsoft-Windows-Sysmon/Operational' '/ms:268435456'
        if ($LASTEXITCODE -ne 0) {
            Write-Warning 'Could not increase the new Sysmon log size; monitoring may still work.'
        }
    }

    $Services = @(Get-Service -Name 'Sysmon', 'Sysmon64' -ErrorAction SilentlyContinue)
    if (@($Services | Where-Object { $_.Status -eq 'Running' }).Count -eq 0) {
        throw 'Sysmon service is not running after setup.'
    }
    & wevtutil.exe gl 'Microsoft-Windows-Sysmon/Operational' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Sysmon Operational event channel was not found.' }

    Write-Host 'Sysmon is running and the Operational channel exists.'
    if ($Existing.Count -gt 0) {
        Write-Warning 'Pre-existing Sysmon configuration was preserved. Habits needs Sysmon Event IDs 1, 3, 5, and 22; enable these manually if absent.'
    }
    exit 0
} catch {
    Write-Error "Sysmon setup failed: $($_.Exception.Message)"
    exit 1
} finally {
    if ($null -ne $TemporaryFolder -and (Test-Path -LiteralPath $TemporaryFolder)) {
        Remove-Item -LiteralPath $TemporaryFolder -Recurse -Force -ErrorAction SilentlyContinue
    }
}
