# cian — offline installer for Windows (x64)
#
# Copies cian.exe to %LOCALAPPDATA%\Programs\cian and adds that folder to your
# user PATH so you can launch it by typing `cian` in any terminal. No admin
# rights and no network access required.
#
# Usage (from this folder):
#   powershell -ExecutionPolicy Bypass -File .\install.ps1

$ErrorActionPreference = "Stop"

$exe = Join-Path $PSScriptRoot "cian.exe"
if (-not (Test-Path $exe)) {
    Write-Error "cian.exe not found next to this script."
    exit 1
}

$dest = Join-Path $env:LOCALAPPDATA "Programs\cian"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item -Path $exe -Destination $dest -Force
Write-Host "Installed cian.exe to $dest"

# Add to the user PATH if it is not already there.
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($null -eq $userPath) { $userPath = "" }
$already = ($userPath -split ';') -contains $dest
if (-not $already) {
    $newPath = if ($userPath.TrimEnd(';') -eq "") { $dest } else { "$($userPath.TrimEnd(';'));$dest" }
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    Write-Host "Added $dest to your user PATH."
    Write-Host "Open a NEW terminal for the change to take effect."
} else {
    Write-Host "$dest is already on your PATH."
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
