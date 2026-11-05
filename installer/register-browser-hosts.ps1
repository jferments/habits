# Register a read-only browser domain journal native messaging host for this user.
# Firefox registration is automatic; Edge requires the ID shown by edge://extensions.
[CmdletBinding()]
param([string]$EdgeExtensionId = '')
$ErrorActionPreference='Stop'
$HostName='org.jferments.habits'
$LocalRoot=Join-Path $env:LOCALAPPDATA 'Habits'
$Binary=Join-Path $LocalRoot 'bin\habbridge.exe'
if (-not (Test-Path -LiteralPath $Binary)) { throw "Missing $Binary" }

$ManifestDir=Join-Path $LocalRoot 'native-host'
New-Item -ItemType Directory -Force -Path $ManifestDir | Out-Null

function Write-Manifest([string]$Path, [object]$Value) {
    $Json=ConvertTo-Json -InputObject $Value -Depth 8
    # PowerShell 5.1's Set-Content UTF8 creates a BOM, which Chromium rejects.
    [System.IO.File]::WriteAllText($Path,$Json,(New-Object System.Text.UTF8Encoding($false)))
}
function Register-Manifest([string]$RegistryKey, [string]$Path) {
    & reg.exe add $RegistryKey /ve /t REG_SZ /d $Path /f | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Could not register native host in $RegistryKey" }
}

$FirefoxPath=Join-Path $ManifestDir "$HostName-firefox.json"
$FirefoxManifest=@{
    name=$HostName;description='Habits local browser domain journal'
    path=$Binary;type='stdio'
    allowed_extensions=@('habits-domain-journal@jferments.local')
}
Write-Manifest $FirefoxPath $FirefoxManifest
Register-Manifest "HKCU\Software\Mozilla\NativeMessagingHosts\$HostName" $FirefoxPath
Write-Host 'Firefox native host registered for the current Windows user.'

$EdgePath=Join-Path $ManifestDir "$HostName-edge.json"
if (-not $EdgeExtensionId -and (Test-Path -LiteralPath $EdgePath)) {
    try {
        $Previous=Get-Content -LiteralPath $EdgePath -Raw | ConvertFrom-Json
        $Origin=@($Previous.allowed_origins)[0]
        if ($Origin -match '^chrome-extension://([a-p]{32})/$') {
            $EdgeExtensionId=$Matches[1]
        }
    } catch { }
}
if ($EdgeExtensionId) {
    if ($EdgeExtensionId -cnotmatch '^[a-p]{32}$') {
        throw 'Invalid Edge extension ID. Copy the 32 lowercase-letter ID from edge://extensions.'
    }
    $EdgeManifest=@{
        name=$HostName;description='Habits local browser domain journal'
        path=$Binary;type='stdio'
        allowed_origins=@("chrome-extension://$EdgeExtensionId/")
    }
    Write-Manifest $EdgePath $EdgeManifest
    Register-Manifest "HKCU\Software\Microsoft\Edge\NativeMessagingHosts\$HostName" $EdgePath
    Write-Host ("Edge native host registered for extension: " + $EdgeExtensionId)
} else {
    Write-Host 'Edge native host NOT yet registered: load the unpacked extension and rerun setup.ps1 -EdgeExtensionId <ID>.'
}
