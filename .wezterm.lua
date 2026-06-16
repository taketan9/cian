-- ~/.wezterm.lua  (これだけを正とする。~/.config/wezterm.lua は削除推奨)
local wezterm = require("wezterm")
local config = wezterm.config_builder()

local is_mac = string.find(wezterm.target_triple, "apple") ~= nil
local is_windows = string.find(wezterm.target_triple, "windows") ~= nil

----------------------------------------------------------------------
-- cian 向け最重要設定: Ctrl 系キーをアプリ(cian)に通す
----------------------------------------------------------------------
config.enable_kitty_keyboard = true

----------------------------------------------------------------------
-- 外観
----------------------------------------------------------------------
config.automatically_reload_config = true
config.color_scheme = "Tokyo Night"
config.font = wezterm.font_with_fallback({ "Hack Nerd Font", "Symbols Nerd Font Mono" })
config.font_size = 16.0
config.line_height = 1.1
config.cell_width = 1.0
config.use_ime = true

config.enable_tab_bar = true
config.show_tabs_in_tab_bar = true
config.hide_tab_bar_if_only_one_tab = true

config.window_background_opacity = 0.75
config.text_background_opacity = 0.90
config.macos_window_background_blur = 20
config.window_decorations = "RESIZE"
config.window_background_gradient = { colors = { "#000000" } }
config.adjust_window_size_when_changing_font_size = false
config.window_close_confirmation = "NeverPrompt"
config.window_padding = { left = 6, right = 6, top = 4, bottom = 4 }

----------------------------------------------------------------------
-- タブの見た目 (2つ目のファイルから移植・修正)
----------------------------------------------------------------------
local SOLID_LEFT_ARROW = wezterm.nerdfonts.ple_lower_right_triangle
local SOLID_RIGHT_ARROW = wezterm.nerdfonts.ple_upper_left_triangle

wezterm.on("format-tab-title", function(tab, tabs, panes, conf, hover, max_width)
  local background = "#5c6d74"
  local foreground = "#FFFFFF"
  local edge_background = "none"
  if tab.is_active then
    background = "#ae8b2d"
    foreground = "#FFFFFF"
  end
  local edge_foreground = background
  local title = "   " .. wezterm.truncate_right(tab.active_pane.title, max_width - 1) .. "   "
  return {
    { Background = { Color = edge_background } },
    { Foreground = { Color = edge_foreground } },
    { Text = SOLID_LEFT_ARROW },
    { Background = { Color = background } },
    { Foreground = { Color = foreground } },
    { Text = title },
    { Background = { Color = edge_background } },
    { Foreground = { Color = edge_foreground } },
    { Text = SOLID_RIGHT_ARROW },
  }
end)

----------------------------------------------------------------------
-- キーバインド
--   cian 使用時、WezTerm 側の keys は cian より優先される点に注意。
--   Ctrl+W / Ctrl+F / Ctrl+T が邪魔になる場合はコメントアウトを。
----------------------------------------------------------------------
local keys = {
  { key = "F11", action = "ToggleFullScreen" },
}

if is_mac then
  table.insert(keys, { key = "h", mods = "CTRL|ALT", action = wezterm.action.ActivatePaneDirection("Left") })
  table.insert(keys, { key = "j", mods = "CTRL|ALT", action = wezterm.action.ActivatePaneDirection("Down") })
  table.insert(keys, { key = "k", mods = "CTRL|ALT", action = wezterm.action.ActivatePaneDirection("Up") })
  table.insert(keys, { key = "l", mods = "CTRL|ALT", action = wezterm.action.ActivatePaneDirection("Right") })
  table.insert(keys, { key = "-", mods = "CTRL", action = wezterm.action.DecreaseFontSize })
  table.insert(keys, { key = "=", mods = "CTRL", action = wezterm.action.IncreaseFontSize })
  table.insert(keys, { key = "t", mods = "CTRL", action = wezterm.action.SpawnTab("CurrentPaneDomain") })
  table.insert(keys, { key = "w", mods = "CTRL", action = wezterm.action.CloseCurrentPane({ confirm = false }) })
  table.insert(keys, { key = "f", mods = "CTRL", action = wezterm.action.Search("CurrentSelectionOrEmptyString") })
  table.insert(keys, { key = "l", mods = "CMD|ALT", action = wezterm.action.ActivateTabRelative(1) })
  table.insert(keys, { key = "h", mods = "CMD|ALT", action = wezterm.action.ActivateTabRelative(-1) })
elseif is_windows then
  table.insert(keys, { key = "h", mods = "CTRL", action = wezterm.action.ActivatePaneDirection("Left") })
  table.insert(keys, { key = "j", mods = "CTRL", action = wezterm.action.ActivatePaneDirection("Down") })
  table.insert(keys, { key = "k", mods = "CTRL", action = wezterm.action.ActivatePaneDirection("Up") })
  table.insert(keys, { key = "l", mods = "CTRL", action = wezterm.action.ActivatePaneDirection("Right") })
  table.insert(keys, { key = "-", mods = "CTRL", action = wezterm.action.DecreaseFontSize })
  table.insert(keys, { key = "=", mods = "CTRL", action = wezterm.action.IncreaseFontSize })
  table.insert(keys, { key = "t", mods = "CTRL", action = wezterm.action.SpawnTab("CurrentPaneDomain") })
  table.insert(keys, { key = "w", mods = "CTRL", action = wezterm.action.CloseCurrentPane({ confirm = false }) })
  table.insert(keys, { key = "f", mods = "CTRL", action = wezterm.action.Search("CurrentSelectionOrEmptyString") })
  table.insert(keys, { key = "l", mods = "CTRL|ALT", action = wezterm.action.ActivateTabRelative(1) })
  table.insert(keys, { key = "h", mods = "CTRL|ALT", action = wezterm.action.ActivateTabRelative(-1) })
end

config.keys = keys

-- cian を常用起動したい場合は次を有効化 (パスは実際の場所に):
-- config.default_prog = { "/usr/local/bin/cian" }

return config
