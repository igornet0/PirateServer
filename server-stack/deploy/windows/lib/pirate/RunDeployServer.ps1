# Runs deploy-server.exe with env from ProgramData pirate-deploy.env
param(
    [string] $InstallDir = "${env:ProgramFiles}\Pirate",
    [string] $DataRoot = "${env:ProgramData}\Pirate",
    [string] $EnvFile = "${env:ProgramData}\Pirate\pirate-deploy.env"
)
$ErrorActionPreference = "Stop"
if (-not (Test-Path $EnvFile)) {
    Write-Error "Missing env file: $EnvFile"
    exit 1
}
Get-Content $EnvFile -Encoding UTF8 | ForEach-Object {
    $line = $_.Trim()
    if ($line -eq "" -or $line.StartsWith("#")) { return }
    $i = $line.IndexOf("=")
    if ($i -lt 1) { return }
    $k = $line.Substring(0, $i).Trim()
    $v = $line.Substring($i + 1).Trim()
    [Environment]::SetEnvironmentVariable($k, $v, "Process")
}
$deployRoot = if ($env:DEPLOY_ROOT) { $env:DEPLOY_ROOT } else { Join-Path $DataRoot "deploy" }
$exe = Join-Path $InstallDir "deploy-server.exe"
if (-not (Test-Path $exe)) { Write-Error "Not found: $exe"; exit 1 }
& $exe --root $deployRoot -p 50051
