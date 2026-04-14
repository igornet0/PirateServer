param([string] $MountPoint)
$ErrorActionPreference = "Stop"
if (-not $MountPoint) { Write-Error "usage: pirate-smb-umount.ps1 <mount_point>"; exit 1 }
if (Test-Path $MountPoint) {
    try { net use $MountPoint /delete /y 2>$null } catch {}
}
