cian — Windows x64 (offline)
============================

This package contains a single self-contained executable. It needs no
installer, no runtime, and no network access.

Contents
  cian.exe          The program (everything is statically linked in).
  install.ps1       Optional: puts cian.exe on your PATH so `cian` works.
  examples\init.lua Optional starter config.

Quick start (no install)
  Just run cian.exe from this folder, or from a terminal:
      .\cian.exe

Install so you can type `cian` anywhere
  Right-click install.ps1 -> "Run with PowerShell", or in a terminal:
      powershell -ExecutionPolicy Bypass -File .\install.ps1
  Then open a NEW terminal and run:
      cian

What it does
  - Copies cian.exe to %LOCALAPPDATA%\Programs\cian
  - Adds that folder to your user PATH (no admin rights needed)
  - If you have no config yet, writes examples\init.lua to
    %USERPROFILE%\.config\cian\init.lua

Notes
  - For the file-type icons, use a terminal with a Nerd Font (Windows Terminal
    or WezTerm are good choices). Without one you'll see boxes instead of icons.
  - Configuration lives at %USERPROFILE%\.config\cian\init.lua
    (override the directory with the CIAN_CONFIG_DIR environment variable).
  - Uninstall: delete %LOCALAPPDATA%\Programs\cian and remove it from your
    user PATH (Settings -> Environment Variables).
