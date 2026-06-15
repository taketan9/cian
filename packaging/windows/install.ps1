<#
cian — offline installer for Windows (x64).

Copies cian.exe to an install folder, clears the "downloaded from the internet"
mark so it runs from a terminal, and adds the folder to PATH so you can launch
it by typing `cian`. No network access required.

Examples:
  # Default: install for the current user (no admin rights needed)
  powershell -ExecutionPolicy Bypass -File .\install.ps1

  # Install into Program Files for all users
  # (run this from an *elevated* PowerShell: Run as administrator)
  powershell -ExecutionPolicy Bypass -File .\install.ps1 -Dest "C:\Program Files\cian" -AllUsers
#>
param(
    # Where to install cian.exe.
    [string]$Dest = (Join-Path $env:LOCALAPPDATA "Programs\cian"),
    # Put the folder on the system (machine) PATH instead of the user PATH.
    [switch]$AllUsers
)

$ErrorActionPreference = "Stop"

$exe = Join-Path $PSScriptRoot "cian.exe"
if (-not (Test-Path $exe)) {
    Write-Error "cian.exe not found next to this script."
    exit 1
}

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole(
        [Security.Principal.WindowsBuiltinRole]::Administrator)
}

$scope = if ($AllUsers) { "Machine" } else { "User" }

# Writing under Program Files, or editing the machine PATH, needs elevation.
$needsAdmin = $AllUsers -or ($Dest -like "$env:ProgramFiles*") `
    -or ($Dest -like "${env:ProgramFiles(x86)}*")
if ($needsAdmin -and -not (Test-Admin)) {
    Write-Error "Installing to '$Dest' (or -AllUsers) needs an elevated PowerShell. Right-click PowerShell -> Run as administrator, then re-run."
    exit 1
}

# Copy and unblock.
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
$destExe = Join-Path $Dest "cian.exe"
Copy-Item -Path $exe -Destination $destExe -Force
Unblock-File -Path $destExe   # so a terminal launch isn't "Access denied"
Write-Host "Installed cian.exe to $Dest"

# If a previous per-user install exists elsewhere, remove it so the terminal
# doesn't pick up a stale copy with a different PATH entry.
$defaultUserDest = Join-Path $env:LOCALAPPDATA "Programs\cian"
if (($Dest -ne $defaultUserDest) -and (Test-Path $defaultUserDest)) {
    Remove-Item -Recurse -Force $defaultUserDest -ErrorAction SilentlyContinue
    $up = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($up) {
        $cleaned = (($up -split ';') | Where-Object { $_ -and ($_ -ne $defaultUserDest) }) -join ';'
        [Environment]::SetEnvironmentVariable("Path", $cleaned, "User")
    }
    Write-Host "Removed a previous install at $defaultUserDest"
}

# Add Dest to the chosen PATH scope (de-duplicated).
$path = [Environment]::GetEnvironmentVariable("Path", $scope)
if ($null -eq $path) { $path = "" }
$parts = ($path -split ';') | Where-Object { $_ }
if ($parts -notcontains $Dest) {
    $parts += $Dest
    [Environment]::SetEnvironmentVariable("Path", ($parts -join ';'), $scope)
    Write-Host "Added $Dest to the $scope PATH. Open a NEW terminal for it to take effect."
} else {
    Write-Host "$Dest is already on the $scope PATH."
}

# Optional: install the example config if the user has none yet.
$cfgDir = Join-Path $env:USERPROFILE ".config\cian"
$cfg = Join-Path $cfgDir "init.lua"
$example = Join-Path $PSScriptRoot "examples\init.lua"
if ((Test-Path $example) -and (-not (Test-Path $cfg))) {
    New-Item -ItemType Directory -Force -Path $cfgDir | Out-Null
    Copy-Item -Path $example -Destination $cfg -Force
    Write-Host "Wrote a starter config to $cfg"
}

Write-Host ""
Write-Host "Done. Open a new terminal and run:  cian"
