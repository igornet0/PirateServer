# Remove Pirate server-stack (Administrator).
param(
    [switch] $ServicesOnly
)
$ErrorActionPreference = "Stop"
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Error "Run as Administrator"
    exit 1
}

$InstallDir = Join-Path $env:ProgramFiles "Pirate"
$DataRoot = Join-Path $env:ProgramData "Pirate"

foreach ($t in @("PirateDeployServer", "PirateControlApi")) {
    try {
        Unregister-ScheduledTask -TaskName $t -Confirm:$false -ErrorAction SilentlyContinue
    } catch {}
}

if (-not $ServicesOnly) {
    if (Test-Path $DataRoot) {
        Remove-Item -Recurse -Force $DataRoot
    }
}

if (Test-Path $InstallDir) {
    Remove-Item -Recurse -Force $InstallDir
}

Write-Host "Uninstall complete."
