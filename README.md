# cian

**C**omfortable **I**nterface for **A**gile File e**X**plorer **N**avigation —
a modern two-pane terminal file manager inspired by [AFXW (あふｗ)](https://akt.d.dooo.jp/akt_afxw.htm).

Runs in any terminal (designed to be used as WezTerm's `default_prog`).
Cross-platform: macOS / Windows / Linux.

## Status

Pre-MVP. Project scaffolding only.

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

## Build

```sh
cargo build --release
./target/release/cian
```
