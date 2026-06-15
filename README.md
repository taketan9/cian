# cian

**C**omfortable **I**nterface for **A**gile File e**X**plorer **N**avigation —
a modern two-pane terminal file manager inspired by [AFXW (あふｗ)](https://akt.d.dooo.jp/akt_afxw.htm).

Runs in any terminal (designed to be used as WezTerm's `default_prog`).
Cross-platform: macOS / Windows / Linux.

## Status

Early development. Working: two-pane navigation, marks/visual selection, file
operations (copy/move/delete/rename/create), history, shortcuts, search,
clipboard integration, Lua configuration, and an embedded PTY shell panel.

## Architecture

Cargo workspace, split into five crates:

| Crate | Role |
|---|---|
| `cian-core` | Pure domain logic: file ops, marks, history, bookmarks, masks, search |
| `cian-tui`  | Rendering & input (ratatui + crossterm), layout, popups |
| `cian-pty`  | Embedded shell pane (portable-pty + alacritty_terminal) |
| `cian-lua`  | Lua configuration host (mlua): keymaps, themes, ext-open DSL |
| `cian-bin`  | Entry point — produces the `cian` binary |

## Configuration

cian reads `~/.config/cian/init.lua` (override the directory with
`$CIAN_CONFIG_DIR`). Configuration is written in Lua via a small WezTerm-style
API on the global `cian` table:

```lua
cian.set_theme({ accent = "#00d7d7", mark_fg = "yellow" })
cian.set_option("clipboard_on_copy", false)
cian.set_keymap("x", "delete")          -- additive override; defaults stay intact
cian.on_open("md", function(path)        -- extension-dispatch execution
  cian.spawn({ "open", "-a", "Typora", path })
end)
```

The file is optional — cian runs with defaults if it is absent. Any syntax or
runtime error is shown in a startup notice and cian falls back to defaults for
whatever could not be applied, so a broken config never blocks startup.

See [`examples/init.lua`](examples/init.lua) for a fully-commented template and
the complete list of bindable actions.

## Shell panel

The bottom panel is a real PTY running your `$SHELL`, started on first focus.
Focus it with `Shift+J` (from a file pane), a mouse click, or `:shell`. While
the shell is focused, keys go straight to it; press **Esc** to return to the
files. Esc is passed through to full-screen programs (vim, less, htop, …) so
they keep working — it only leaves the shell at a normal prompt.

Shell tabs are driven by function keys (Ctrl-based shortcuts are unreliable
because some setups swallow the Ctrl modifier before it reaches the app):

| Key | Action |
|---|---|
| `F1`–`F8` | switch to shell tab 1–8 |
| `F9` | new shell tab |
| `F10` | close shell tab |
| `Shift+F1` / `Shift+F2` | focus next / previous split pane |
| `Shift+F8` | split the active pane left/right |
| `Shift+F9` | split the active pane top/bottom |
| `Shift+F10` | close the active split pane (asks first) |
| `F12` | zoom the focused surface to fill the window (toggle) |
| `Shift+F12` | zoom just the active split pane (toggle) |

Splits nest: splitting always divides the active pane, so you can build
arbitrary layouts (e.g. one pane on the left, two stacked on the right). These
keys are only active at a normal prompt; full-screen apps (vim, htop, …)
receive the function keys unchanged.

The file panes use the parallel controls: `Shift+F1` / `Shift+F2` switch to the
next / previous tab, and `Shift+F10` closes the active tab (asking first).

## Build

```sh
cargo build --release
./target/release/cian
```

## Install on Windows (offline)

cian compiles to a single self-contained `cian.exe` — no runtime, no DLLs, no
network access needed at runtime. To get a Windows x64 build without a Windows
dev machine, use the bundled GitHub Actions workflow, which builds on a real
Windows runner and packages a ready-to-carry zip:

1. Trigger a build — either push a tag (`git tag v0.1.0 && git push --tags`) or
   open the repo's **Actions** tab → **release** → **Run workflow**.
2. Download `cian-windows-x64.zip` from that run's artifacts (tagged builds are
   also attached to a GitHub Release).
3. Carry the zip into the offline machine and unzip it. Then either just run
   `cian.exe`, or run `install.ps1` to put `cian` on your PATH:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```

   The default installs for the current user (no admin) under
   `%LOCALAPPDATA%\Programs\cian`. To install into Program Files for all users,
   run an **elevated** PowerShell and pass a destination:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1 -Dest "C:\Program Files\cian" -AllUsers
   ```

   The installer unblocks the exe (so a terminal launch isn't "Access denied")
   and adds the folder to PATH. Open a new terminal and type `cian`. Use a Nerd
   Font terminal (Windows Terminal / WezTerm) for the file-type icons.
