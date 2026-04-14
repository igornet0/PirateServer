# v1: SMB mount from control-api is not implemented on Windows (see README).
param([string] $MountPoint, [string] $Unc, [string] $Cred)
$ErrorActionPreference = "Stop"
Write-Host "pirate-smb-mount: Windows v1 — map SMB manually (net use / New-SmbMapping)." -ForegroundColor Red
exit 1
