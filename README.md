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

## Build

```sh
cargo build --release
./target/release/cian
```
