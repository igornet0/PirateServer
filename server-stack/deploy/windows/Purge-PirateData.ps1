# Remove data under ProgramData\Pirate (Administrator).
param(
    [switch] $RemovePostgres
)
$ErrorActionPreference = "Stop"
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Error "Run as Administrator"
    exit 1
}

$DataRoot = Join-Path $env:ProgramData "Pirate"
if (Test-Path $DataRoot) {
    Remove-Item -Recurse -Force $DataRoot
}
Write-Host "Purge complete."
if ($RemovePostgres) {
    Write-Warning "-RemovePostgres: drop databases manually if PostgreSQL is installed."
}
