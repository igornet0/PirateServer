# OTA apply for Windows (run elevated). Analog of pirate-apply-stack-bundle.sh on Unix.
# Usage: pirate-apply-stack-bundle.ps1 <bundle_root_abs> <version_label> [apply_options.json]
param(
    [Parameter(Mandatory = $true)][string] $BundleRoot,
    [Parameter(Mandatory = $true)][string] $VersionLabel,
    [string] $ApplyJsonPath = ""
)
$ErrorActionPreference = "Stop"
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Error "pirate-apply-stack-bundle.ps1: must run as Administrator"
    exit 1
}

$DataRoot = Join-Path $env:ProgramData "Pirate"
$pr = [System.IO.Path]::GetFullPath($DataRoot)
$br = [System.IO.Path]::GetFullPath($BundleRoot)
if (-not $br.StartsWith($pr, [StringComparison]::OrdinalIgnoreCase)) {
    Write-Error "bundle_root must be under $DataRoot"
    exit 1
}
if (-not (Test-Path $br -PathType Container)) {
    Write-Error "not a directory: $br"
    exit 1
}

$BinDir = Join-Path $br "bin"
$ds = Join-Path $BinDir "deploy-server.exe"
$ca = Join-Path $BinDir "control-api.exe"
$cl = Join-Path $BinDir "client.exe"
if (-not ((Test-Path $ds) -and (Test-Path $ca) -and (Test-Path $cl))) {
    Write-Error "missing binaries under $BinDir"
    exit 1
}

$InstallDir = Join-Path ${env:ProgramFiles} "Pirate"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

Write-Host "==> install binaries -> $InstallDir"
Copy-Item -Force $ds (Join-Path $InstallDir "deploy-server.exe")
Copy-Item -Force $ca (Join-Path $InstallDir "control-api.exe")
Copy-Item -Force $cl (Join-Path $InstallDir "client.exe")
$pi = Join-Path $BinDir "pirate.exe"
if (Test-Path $pi) {
    Copy-Item -Force $pi (Join-Path $InstallDir "pirate.exe")
} else {
    Copy-Item -Force $cl (Join-Path $InstallDir "pirate.exe")
}

$UiSrc = Join-Path $br "share\ui\dist"
if (Test-Path (Join-Path $UiSrc "index.html")) {
    Write-Host "==> frontend -> $DataRoot\ui\dist"
    $uiDest = Join-Path $DataRoot "ui\dist"
    if (Test-Path $uiDest) { Remove-Item -Recurse -Force $uiDest }
    New-Item -ItemType Directory -Force -Path (Split-Path $uiDest) | Out-Null
    Copy-Item -Recurse -Force $UiSrc $uiDest
}

$manifest = Join-Path $br "server-stack-manifest.json"
if (Test-Path $manifest) {
    Copy-Item -Force $manifest (Join-Path $DataRoot "server-stack-manifest.json")
}

$verFile = Join-Path $DataRoot "server-stack-version"
Set-Content -Path $verFile -Value $VersionLabel -Encoding UTF8

$NewApply = Join-Path $br "lib\pirate\pirate-apply-stack-bundle.ps1"
if (Test-Path $NewApply) {
    $libPirate = Join-Path $InstallDir "lib\pirate"
    New-Item -ItemType Directory -Force -Path $libPirate | Out-Null
    Copy-Item -Force $NewApply (Join-Path $libPirate "pirate-apply-stack-bundle.ps1")
}

function Read-DeployEnv {
    param([string]$Path)
    $d = @{}
    if (-not (Test-Path $Path)) { return $d }
    Get-Content $Path -Encoding UTF8 | ForEach-Object {
        $line = $_.Trim()
        if ($line -eq "" -or $line.StartsWith("#")) { return }
        $i = $line.IndexOf("=")
        if ($i -lt 1) { return }
        $d[$line.Substring(0, $i).Trim()] = $line.Substring($i + 1).Trim()
    }
    return $d
}

function Write-DeployEnv {
    param([string]$Path, [hashtable]$Map)
    $order = @(
        "DEPLOY_SQLITE_URL", "DEPLOY_ROOT", "GRPC_ENDPOINT", "CONTROL_API_PORT", "RUST_LOG",
        "CONTROL_API_BIND", "DEPLOY_ALLOW_SERVER_STACK_UPDATE", "CONTROL_API_HOST_STATS_SERIES",
        "CONTROL_API_HOST_STATS_STREAM", "CONTROL_UI_ADMIN_USERNAME", "CONTROL_UI_ADMIN_PASSWORD",
        "CONTROL_API_JWT_SECRET", "DEPLOY_GRPC_PUBLIC_URL", "GRPC_SIGNING_KEY_PATH"
    )
    $lines = New-Object System.Collections.ArrayList
    $seen = @{}
    foreach ($k in $order) {
        if ($Map.ContainsKey($k)) {
            [void]$lines.Add("$k=$($Map[$k])")
            $seen[$k] = $true
        }
    }
    foreach ($k in ($Map.Keys | Sort-Object)) {
        if (-not $seen.ContainsKey($k)) { [void]$lines.Add("$k=$($Map[$k])") }
    }
    Set-Content -Path $Path -Value ($lines -join "`n") -Encoding UTF8
}

$EnvPath = Join-Path $env:ProgramData "Pirate\pirate-deploy.env"
$modeEnableUi = $false

if ($ApplyJsonPath -ne "" -and (Test-Path $ApplyJsonPath)) {
    Write-Host "==> stack apply options (OTA UI transition)"
    $j = Get-Content $ApplyJsonPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $mode = [string]$j.mode
    if ($mode -eq "disable_ui") {
        $uiPath = Join-Path $DataRoot "ui\dist"
        if (Test-Path $uiPath) { Remove-Item -Recurse -Force $uiPath }
        $d = Read-DeployEnv $EnvPath
        @("CONTROL_UI_ADMIN_USERNAME", "CONTROL_UI_ADMIN_PASSWORD", "CONTROL_API_JWT_SECRET") | ForEach-Object {
            if ($d.ContainsKey($_)) { $d.Remove($_) }
        }
        Write-DeployEnv $EnvPath $d
    }
    elseif ($mode -eq "enable_ui") {
        $modeEnableUi = $true
        $d = Read-DeployEnv $EnvPath
        $dr = Join-Path $DataRoot "deploy"
        if (-not $d["DEPLOY_SQLITE_URL"]) { $d["DEPLOY_SQLITE_URL"] = "sqlite:///$($dr.Replace('\', '/'))/deploy.db" }
        if (-not $d["DEPLOY_ROOT"]) { $d["DEPLOY_ROOT"] = $dr.Replace("\", "/") }
        if (-not $d["GRPC_ENDPOINT"]) { $d["GRPC_ENDPOINT"] = "http://[::1]:50051" }
        if (-not $d["CONTROL_API_PORT"]) { $d["CONTROL_API_PORT"] = "8080" }
        if (-not $d["RUST_LOG"]) { $d["RUST_LOG"] = "info" }
        if (-not $d["CONTROL_API_BIND"]) { $d["CONTROL_API_BIND"] = "127.0.0.1" }
        $d["CONTROL_UI_ADMIN_USERNAME"] = if ($j.ui_admin_username) { [string]$j.ui_admin_username } else { "admin" }
        if ($j.ui_admin_password) {
            $d["CONTROL_UI_ADMIN_PASSWORD"] = [string]$j.ui_admin_password
        } else {
            $d["CONTROL_UI_ADMIN_PASSWORD"] = -join ((65..90) + (97..122) + (48..57) | Get-Random -Count 24 | ForEach-Object { [char]$_ })
        }
        $bytes = New-Object byte[] 48
        [System.Security.Cryptography.RNGCryptoServiceProvider]::Create().GetBytes($bytes)
        $d["CONTROL_API_JWT_SECRET"] = [Convert]::ToBase64String($bytes)
        $dom = if ($j.domain) { [string]$j.domain } else { "" }
        if ($dom) {
            $d["DEPLOY_GRPC_PUBLIC_URL"] = "http://${dom}:50051"
        } else {
            $d["DEPLOY_GRPC_PUBLIC_URL"] = "http://127.0.0.1:50051"
        }
        Write-DeployEnv $EnvPath $d
        if ($j.install_nginx) {
            Write-Warning "enable_ui: nginx install on Windows is not automated; configure nginx for Windows manually (see README)."
        }
    }
}

function Restart-PirateTasks {
    foreach ($t in @("PirateDeployServer", "PirateControlApi")) {
        try { Stop-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue } catch {}
    }
    Start-Sleep -Seconds 2
    try { Start-ScheduledTask -TaskName "PirateDeployServer" } catch {}
    Start-Sleep -Seconds 5
    try { Start-ScheduledTask -TaskName "PirateControlApi" } catch {}
}

if ($ApplyJsonPath -ne "" -and (Test-Path $ApplyJsonPath) -and $modeEnableUi) {
    Write-Host "==> control-api bootstrap-grpc-key (enable_ui)"
    $deployRoot = Join-Path $DataRoot "deploy"
    $env:DEPLOY_ROOT = $deployRoot
    & (Join-Path $InstallDir "control-api.exe") bootstrap-grpc-key 2>$null
    $d = Read-DeployEnv $EnvPath
    $d["GRPC_SIGNING_KEY_PATH"] = (Join-Path $deployRoot ".keys\control_api_ed25519.json").Replace("\", "/")
    Write-DeployEnv $EnvPath $d
}

Write-Host "==> schedule service restarts (scheduled tasks)"
Restart-PirateTasks

Write-Host "ok: server-stack $VersionLabel staged; tasks restarted"
