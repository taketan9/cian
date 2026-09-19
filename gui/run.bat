@echo off
setlocal enabledelayedexpansion
rem Launch cian (the Electron front end).
rem
rem ASCII only, deliberately. cmd.exe tracks its read position in this file by
rem byte offset; `chcp 65001` partway through changes how many bytes a
rem character takes, and every line after it is read from the wrong place. A
rem comment then splits in the middle and its tail runs as a command:
rem
rem     '...I is not recognized as an internal or external command
rem
rem That is what a Japanese comment in here did on 2026-08-31. The messages
rem below stay English so the file can never do it again; GUI.txt carries the
rem Japanese, and a file that is only ever read cannot be mis-parsed.
rem
rem What this does: find Electron, then hand it this folder. Looks in the
rem usual places and, when it finds nothing, says every place it looked --
rem failing silently is the worst of the options.

set "HERE=%~dp0"
set "HERE=%HERE:~0,-1%"
set "FOUND="

rem 1. An environment variable wins, for trying another build without
rem    disturbing the settled one.
if defined CIAN_ELECTRON (
    if exist "%CIAN_ELECTRON%" set "FOUND=%CIAN_ELECTRON%"
)

rem 2. electron.txt beside this file.
rem
rem Not shipped in the zip on purpose: a file that ships is a file the next
rem version overwrites, which is the whole reason this exists instead of
rem editing run.bat. One line, either the exe or the folder holding it.
rem Lines starting with ; are skipped, so it can carry a note.
if not defined FOUND (
    if exist "%HERE%\electron.txt" (
        for /f "usebackq delims=" %%L in ("%HERE%\electron.txt") do (
            if not defined FOUND (
                if exist "%%~L\electron.exe" (
                    set "FOUND=%%~L\electron.exe"
                ) else if exist "%%~L" (
                    set "FOUND=%%~L"
                )
            )
        )
    )
)

rem 3. Unpacked right here, or one level up.
if not defined FOUND (
    for /d %%D in ("%HERE%\electron-v*" "%HERE%\..\electron-v*") do (
        if exist "%%D\electron.exe" set "FOUND=%%D\electron.exe"
    )
)

rem 4. Beside the repository, which is how it looks when working from source.
if not defined FOUND (
    for /d %%D in ("%HERE%\..\..\electron-v*") do (
        if exist "%%D\electron.exe" set "FOUND=%%D\electron.exe"
    )
)

rem 5. Installed with npm.
if not defined FOUND (
    if exist "%HERE%\node_modules\electron\dist\electron.exe" (
        set "FOUND=%HERE%\node_modules\electron\dist\electron.exe"
    )
)

if not defined FOUND (
    echo Electron not found. Looked in:
    echo   1. %%CIAN_ELECTRON%%  ^(now: "%CIAN_ELECTRON%"^)
    echo   2. the path written in %HERE%\electron.txt
    echo   3. %HERE%\electron-v*\electron.exe
    echo      %HERE%\..\electron-v*\electron.exe
    echo   4. %HERE%\..\..\electron-v*\electron.exe
    echo   5. %HERE%\node_modules\electron\dist\electron.exe
    echo.
    echo Easiest fix: make electron.txt in this folder with one line in it,
    echo pointing at Electron. Notepad will do. For example:
    echo   D:\apps\electron-v33.4.11-win32-x64
    echo.
    echo Do not edit run.bat instead -- it works, but the next version you
    echo unpack overwrites it. electron.txt is not in the zip, so it survives.
    echo See GUI.txt for the same thing in Japanese.
    pause
    exit /b 1
)

rem ---------------------------------------------------------------------
rem Shared install: give a first-time user the team's settings.
rem
rem For a shared machine (an AVD image, a Citrix host) where this folder sits
rem in Program Files and everybody launches the same run.bat from the common
rem desktop.
rem
rem   default-config\*.lua   copied only when the user has none.
rem
rem One rule: **first launch only**. After that every file is that person's,
rem including ssh.lua, and nothing here touches it again. A settings file that
rem is rewritten under somebody while they are using it is a settings file
rem they cannot trust -- and the first thing anyone does with a server list is
rem add the one host that is missing from it.
rem
rem NEVER put init.lua next to this file to distribute it. cian reads a config
rem beside the executable first -- and *writes* there too once one exists, so
rem bookmarks (shortcuts.lua) would be saved into Program Files, where a
rem standard user cannot write. Everybody's bookmarks would fail, quietly.
rem `:where` inside cian says which file it is reading and where it would
rem write; that is the way to check a deployment.
set "SEED=%HERE%\default-config"
set "MINE=%USERPROFILE%\.config\cian"
if defined CIAN_CONFIG_DIR set "MINE=%CIAN_CONFIG_DIR%"
if exist "%SEED%" (
    if not exist "%MINE%" mkdir "%MINE%" 2>nul
    for %%F in ("%SEED%\*.lua") do (
        if not exist "%MINE%\%%~nxF" copy /y "%%F" "%MINE%\" >nul
    )
)

rem The mistake above, caught rather than explained. A config file here means
rem this folder is where cian will try to save to.
for %%N in (init.lua shortcuts.lua macro.lua) do (
    if exist "%HERE%\%%N" (
        echo WARNING: %%N is next to the program.
        echo cian will read it -- and will also try to SAVE bookmarks here.
        echo On a shared install put it in default-config\ instead.
        echo.
    )
)

rem Say so before showing an empty window, rather than after.
if not exist "%HERE%\cian-server.exe" (
    if not exist "%HERE%\..\target\release\cian-server.exe" (
        if not exist "%HERE%\..\target\debug\cian-server.exe" (
            echo cian-server.exe is missing. Either:
            echo   - unzip the release's cian-server-win-x64.exe.zip and put it at
            echo     "%HERE%\cian-server.exe"
            echo   - or build it: cargo build --release -p cian-server
            pause
            exit /b 1
        )
    )
)

rem `start` rather than running it here, so this console window goes away.
rem
rem Run in the foreground, cmd.exe stays open for as long as cian does, and
rem that console is a second taskbar button next to the window -- which is
rem half of the "two windows open" this was reported as. Everything that can
rem be checked has been checked by now, so there is nothing left for the
rem console to say.
rem
rem The empty "" is the window title `start` insists on before the command;
rem without it, a quoted path is read as the title and nothing is launched.
echo Electron: %FOUND%
start "" "%FOUND%" "%HERE%" %*
exit /b 0
