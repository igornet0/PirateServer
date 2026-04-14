# Install Pirate server-stack on Windows (Administrator).
# Usage: .\Install.ps1 [-Nginx] [-Ui] [-Domain fqdn] [-NonInteractive]
# Default: binaries + scheduled tasks; -Nginx documents manual nginx; -Ui copies dashboard static files.

param(
    [switch] $Nginx,
    [switch] $Ui,
    [string] $Domain = "",
    [switch] $NonInteractive
)

$ErrorActionPreference = "Stop"
$BundleRoot = $PSScriptRoot
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Error "Run as Administrator: right-click PowerShell -> Run as administrator"
    exit 1
}

$NoUi = Test-Path (Join-Path $BundleRoot ".bundle-no-ui")
if ($Ui -and $NoUi) {
    Write-Error "This bundle has no dashboard static (.bundle-no-ui); -Ui is not allowed."
    exit 1
}

$BinLocal = Join-Path $BundleRoot "bin"
foreach ($f in @("deploy-server.exe", "control-api.exe", "client.exe")) {
    if (-not (Test-Path (Join-Path $BinLocal $f))) {
        Write-Error "Missing $f — run from extracted pirate-windows-* folder."
        exit 1
    }
}
if ($Ui -and -not (Test-Path (Join-Path $BundleRoot "share\ui\dist\index.html"))) {
    Write-Error "Missing share\ui\dist for -Ui"
    exit 1
}

$InstallDir = Join-Path $env:ProgramFiles "Pirate"
$DataRoot = Join-Path $env:ProgramData "Pirate"
$DeployRoot = Join-Path $DataRoot "deploy"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
New-Item -ItemType Directory -Force -Path $DeployRoot | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $DataRoot "db-mounts\.creds") -Force | Out-Null

Write-Host "==> install binaries -> $InstallDir"
Copy-Item -Force (Join-Path $BinLocal "deploy-server.exe") (Join-Path $InstallDir "deploy-server.exe")
Copy-Item -Force (Join-Path $BinLocal "control-api.exe") (Join-Path $InstallDir "control-api.exe")
Copy-Item -Force (Join-Path $BinLocal "client.exe") (Join-Path $InstallDir "client.exe")
if (Test-Path (Join-Path $BinLocal "pirate.exe")) {
    Copy-Item -Force (Join-Path $BinLocal "pirate.exe") (Join-Path $InstallDir "pirate.exe")
} else {
    Copy-Item -Force (Join-Path $BinLocal "client.exe") (Join-Path $InstallDir "pirate.exe")
}

$LibSrc = Join-Path $BundleRoot "lib\pirate"
$LibDst = Join-Path $InstallDir "lib\pirate"
New-Item -ItemType Directory -Force -Path $LibDst | Out-Null
Get-ChildItem $LibSrc -Filter "*.ps1" | ForEach-Object { Copy-Item -Force $_.FullName (Join-Path $LibDst $_.Name) }

if ($Ui) {
    Write-Host "==> frontend -> $DataRoot\ui\dist"
    $uiDest = Join-Path $DataRoot "ui\dist"
    if (Test-Path $uiDest) { Remove-Item -Recurse -Force $uiDest }
    Copy-Item -Recurse -Force (Join-Path $BundleRoot "share\ui\dist") $uiDest
}

if (Test-Path (Join-Path $BundleRoot "server-stack-manifest.json")) {
    Copy-Item -Force (Join-Path $BundleRoot "server-stack-manifest.json") (Join-Path $DataRoot "server-stack-manifest.json")
}

$db = Join-Path $DeployRoot "deploy.db"
if (-not (Test-Path $db)) { New-Item -ItemType File -Path $db -Force | Out-Null }

$EnvPath = Join-Path $DataRoot "pirate-deploy.env"
$publicUrl = "http://127.0.0.1:50051"
if ($Domain) { $publicUrl = "http://${Domain}:50051" }

$jwt = ""
$uiUser = "admin"
$uiPass = ""
if ($Ui) {
    $bytes = New-Object byte[] 48
    [System.Security.Cryptography.RNGCryptoServiceProvider]::Create().GetBytes($bytes)
    $jwt = [Convert]::ToBase64String($bytes)
    if (-not $NonInteractive) {
        $uiUser = Read-Host "Dashboard username [admin]"
        if ([string]::IsNullOrWhiteSpace($uiUser)) { $uiUser = "admin" }
        $sec = Read-Host "Dashboard password (empty = random)" -AsSecureString
        $uiPass = if ($sec.Length -gt 0) {
            [Runtime.InteropServices.Marshal]::PtrToStringAuto([Runtime.InteropServices.Marshal]::SecureStringToBSTR($sec))
        } else {
            -join ((48..57 + 65..90 + 97..122) | Get-Random -Count 24 | ForEach-Object { [char]$_ })
        }
    } else {
        $uiPass = -join ((48..57 + 65..90 + 97..122) | Get-Random -Count 24 | ForEach-Object { [char]$_ })
    }
}

$sqliteUrl = "sqlite:///$($DeployRoot.Replace('\', '/'))/deploy.db"
@"
DEPLOY_SQLITE_URL=$sqliteUrl
DEPLOY_ROOT=$($DeployRoot.Replace('\', '/'))
GRPC_ENDPOINT=http://[::1]:50051
CONTROL_API_PORT=8080
RUST_LOG=info
CONTROL_API_BIND=127.0.0.1
DEPLOY_ALLOW_SERVER_STACK_UPDATE=0
CONTROL_API_HOST_STATS_SERIES=0
CONTROL_API_HOST_STATS_STREAM=0
DEPLOY_GRPC_PUBLIC_URL=$publicUrl
"@ | Set-Content -Path $EnvPath -Encoding UTF8

if ($Ui) {
    Add-Content -Path $EnvPath -Value "CONTROL_UI_ADMIN_USERNAME=$uiUser`nCONTROL_UI_ADMIN_PASSWORD=$uiPass`nCONTROL_API_JWT_SECRET=$jwt`n" -Encoding UTF8
}

$psExe = Join-Path $env:WINDIR "System32\WindowsPowerShell\v1.0\powershell.exe"
$runDs = Join-Path $LibDst "RunDeployServer.ps1"
$runCa = Join-Path $LibDst "RunControlApi.ps1"
$argDs = "-NoProfile -ExecutionPolicy Bypass -File `"$runDs`""
$argCa = "-NoProfile -ExecutionPolicy Bypass -File `"$runCa`""

$actionDs = New-ScheduledTaskAction -Execute $psExe -Argument $argDs
$triggerDs = New-ScheduledTaskTrigger -AtStartup
$principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Hours 0)
Register-ScheduledTask -TaskName "PirateDeployServer" -Action $actionDs -Trigger $triggerDs -Principal $principal -Settings $settings -Force | Out-Null

$actionCa = New-ScheduledTaskAction -Execute $psExe -Argument $argCa
$triggerCa = New-ScheduledTaskTrigger -AtStartup
Register-ScheduledTask -TaskName "PirateControlApi" -Action $actionCa -Trigger $triggerCa -Principal $principal -Settings $settings -Force | Out-Null

Write-Host "==> start deploy-server task"
Start-ScheduledTask -TaskName "PirateDeployServer"
Start-Sleep -Seconds 6

Write-Host "==> bootstrap-grpc-key"
$env:DEPLOY_ROOT = $DeployRoot
& (Join-Path $InstallDir "control-api.exe") bootstrap-grpc-key
$keyLine = "GRPC_SIGNING_KEY_PATH=$($DeployRoot.Replace('\', '/'))/.keys/control_api_ed25519.json"
Add-Content -Path $EnvPath -Value $keyLine -Encoding UTF8

Write-Host "==> start control-api task"
Start-ScheduledTask -TaskName "PirateControlApi"

if ($Nginx) {
    Write-Warning "-Nginx: configure nginx for Windows manually; templates are under the bundle nginx\ folder and README."
}

Write-Host ""
Write-Host "Done. Health: curl http://127.0.0.1:8080/health"
if ($Ui) { Write-Host "Dashboard user: $uiUser  password: $uiPass" }
