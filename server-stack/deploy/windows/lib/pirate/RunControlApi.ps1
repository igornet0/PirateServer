# Runs control-api.exe with env from pirate-deploy.env
param(
    [string] $InstallDir = "${env:ProgramFiles}\Pirate",
    [string] $DataRoot = "${env:ProgramData}\Pirate",
    [string] $EnvFile = "${env:ProgramData}\Pirate\pirate-deploy.env"
)
$ErrorActionPreference = "Stop"
# Give deploy-server time to listen before control-api (scheduled task at boot).
Start-Sleep -Seconds 20
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
$exe = Join-Path $InstallDir "control-api.exe"
if (-not (Test-Path $exe)) { Write-Error "Not found: $exe"; exit 1 }
& $exe --deploy-root $deployRoot --listen-port 8080
