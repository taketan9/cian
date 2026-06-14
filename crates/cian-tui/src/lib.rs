use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Result;
use cian_core::Pane;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

// cian accent color (cian-blue, kept consistent across the app)
const CIAN_ACCENT: Color = Color::Cyan;
const STATUS_BG: Color = Color::Rgb(40, 40, 55);
const SELECTED_BG: Color = Color::Rgb(60, 60, 90);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPane {
    Left,
    Right,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Visual,
    Search,
    Command,
    Shell,
}

impl Mode {
    fn short(self) -> &'static str {
        match self {
            Mode::Normal => "NOR",
            Mode::Visual => "VIS",
            Mode::Search => "SEA",
            Mode::Command => "CMD",
            Mode::Shell => "SH ",
        }
    }
}

/// A tab strip within a single pane container.
/// Each tab is an independent `Pane` (cwd + entries + cursor).
pub struct PaneTabs {
    pub tabs: Vec<Pane>,
    pub active: usize,
}

impl PaneTabs {
    pub fn single(p: Pane) -> Self {
        Self { tabs: vec![p], active: 0 }
    }

    pub fn active_ref(&self) -> &Pane {
        &self.tabs[self.active]
    }
    pub fn active_mut(&mut self) -> &mut Pane {
        &mut self.tabs[self.active]
    }

    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }
    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
        }
    }
    pub fn select(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.active = idx;
        }
    }
    pub fn add_clone(&mut self) -> Result<()> {
        let cwd = self.active_ref().cwd.clone();
        let new_pane = Pane::new(cwd)?;
        self.tabs.push(new_pane);
        self.active = self.tabs.len() - 1;
        Ok(())
    }
    pub fn close_active(&mut self) {
        if self.tabs.len() > 1 {
            self.tabs.remove(self.active);
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len() - 1;
            }
        }
    }
}

/// Placeholder shell tabs. Real PTY integration arrives in sprint 4.
pub struct ShellTabs {
    pub count: usize,
    pub active: usize,
}
impl ShellTabs {
    fn new() -> Self {
        Self { count: 1, active: 0 }
    }
    fn next_tab(&mut self) {
        if self.count > 0 {
            self.active = (self.active + 1) % self.count;
        }
    }
    fn prev_tab(&mut self) {
        if self.count > 0 {
            self.active = (self.active + self.count - 1) % self.count;
        }
    }
    fn select(&mut self, idx: usize) {
        if idx < self.count {
            self.active = idx;
        }
    }
    fn add(&mut self) {
        self.count += 1;
        self.active = self.count - 1;
    }
    fn close_active(&mut self) {
        if self.count > 1 {
            self.count -= 1;
            if self.active >= self.count {
                self.active = self.count - 1;
            }
        }
    }
}

pub struct App {
    pub left: PaneTabs,
    pub right: PaneTabs,
    pub shell: ShellTabs,
    pub focused: FocusedPane,
    pub mode: Mode,
    pub mask: String,
    pub command_buffer: String,
    pub message: Option<String>,
    pub last_file_pane: FocusedPane,
    pub should_quit: bool,
}

impl App {
    pub fn new(left: PathBuf, right: PathBuf) -> Result<Self> {
        Ok(Self {
            left: PaneTabs::single(Pane::new(left)?),
            right: PaneTabs::single(Pane::new(right)?),
            shell: ShellTabs::new(),
            focused: FocusedPane::Left,
            mode: Mode::Normal,
            mask: "*.*".to_string(),
            command_buffer: String::new(),
            message: None,
            last_file_pane: FocusedPane::Left,
            should_quit: false,
        })
    }

    fn active_file_tabs(&self) -> Option<&PaneTabs> {
        match self.focused {
            FocusedPane::Left => Some(&self.left),
            FocusedPane::Right => Some(&self.right),
            FocusedPane::Shell => None,
        }
    }
    fn active_file_tabs_mut(&mut self) -> Option<&mut PaneTabs> {
        match self.focused {
            FocusedPane::Left => Some(&mut self.left),
            FocusedPane::Right => Some(&mut self.right),
            FocusedPane::Shell => None,
        }
    }
    fn active_pane(&self) -> Option<&Pane> {
        self.active_file_tabs().map(|t| t.active_ref())
    }
    fn active_pane_mut(&mut self) -> Option<&mut Pane> {
        self.active_file_tabs_mut().map(|t| t.active_mut())
    }

    fn focus(&mut self, target: FocusedPane) {
        if matches!(self.focused, FocusedPane::Left | FocusedPane::Right) {
            self.last_file_pane = self.focused;
        }
        self.focused = target;
        self.mode = match target {
            FocusedPane::Shell => Mode::Shell,
            _ => Mode::Normal,
        };
    }

    fn focus_direction(&mut self, dir: char) {
        let next = match (self.focused, dir) {
            (FocusedPane::Left, 'l') => FocusedPane::Right,
            (FocusedPane::Right, 'h') => FocusedPane::Left,
            (FocusedPane::Left | FocusedPane::Right, 'j') => FocusedPane::Shell,
            (FocusedPane::Shell, 'k') => self.last_file_pane,
            (FocusedPane::Shell, 'h') => FocusedPane::Left,
            (FocusedPane::Shell, 'l') => FocusedPane::Right,
            _ => self.focused,
        };
        if next != self.focused {
            self.focus(next);
        }
    }

    fn run_command(&mut self) {
        let raw = self.command_buffer.trim().to_string();
        self.command_buffer.clear();
        self.mode = Mode::Normal;
        match raw.as_str() {
            "" => {}
            "q" | "quit" => self.should_quit = true,
            "shell" => self.focus(FocusedPane::Shell),
            other => {
                self.message = Some(format!("unknown command: :{}", other));
            }
        }
    }

    fn open_externally(&mut self) {
        let Some(pane) = self.active_pane() else { return };
        let Some(entry) = pane.selected() else { return };
        let path = entry.path.clone();
        match os_open(&path) {
            Ok(()) => self.message = Some(format!("opened: {}", path.display())),
            Err(e) => self.message = Some(format!("open failed: {}", e)),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // command mode swallows all input
        if self.mode == Mode::Command {
            match key.code {
                KeyCode::Esc => {
                    self.command_buffer.clear();
                    self.mode = Mode::Normal;
                }
                KeyCode::Enter => self.run_command(),
                KeyCode::Backspace => {
                    self.command_buffer.pop();
                }
                KeyCode::Char(c) => self.command_buffer.push(c),
                _ => {}
            }
            return Ok(());
        }

        // shell pane: only minimal navigation works (real shell arrives in sprint 4)
        if self.focused == FocusedPane::Shell {
            match (key.modifiers, key.code) {
                (m, KeyCode::Char('h')) if m.contains(KeyModifiers::CONTROL) => {
                    self.focus_direction('h')
                }
                (m, KeyCode::Char('l')) if m.contains(KeyModifiers::CONTROL) => {
                    self.focus_direction('l')
                }
                (m, KeyCode::Char('k')) if m.contains(KeyModifiers::CONTROL) => {
                    self.focus_direction('k')
                }
                (KeyModifiers::NONE, KeyCode::Tab) => self.shell.next_tab(),
                (KeyModifiers::SHIFT, KeyCode::BackTab) => self.shell.prev_tab(),
                (m, KeyCode::Char(c)) if m.contains(KeyModifiers::CONTROL) && c.is_ascii_digit() => {
                    if let Some(d) = c.to_digit(10) {
                        if d >= 1 {
                            self.shell.select((d as usize) - 1);
                        }
                    }
                }
                (m, KeyCode::Char('t')) if m.contains(KeyModifiers::CONTROL) => self.shell.add(),
                (m, KeyCode::Char('w')) if m.contains(KeyModifiers::CONTROL) => {
                    self.shell.close_active()
                }
                (KeyModifiers::NONE, KeyCode::Esc) => self.focus(self.last_file_pane),
                _ => {}
            }
            return Ok(());
        }

        // file pane: normal mode
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (ctrl, key.code) {
            (false, KeyCode::Char('q')) => self.should_quit = true,
            (false, KeyCode::Char(':')) => {
                self.mode = Mode::Command;
                self.command_buffer.clear();
            }
            // pane navigation
            (true, KeyCode::Char('h')) => self.focus_direction('h'),
            (true, KeyCode::Char('j')) => self.focus_direction('j'),
            (true, KeyCode::Char('k')) => self.focus_direction('k'),
            (true, KeyCode::Char('l')) => self.focus_direction('l'),
            // tab management
            (false, KeyCode::Tab) => {
                if let Some(t) = self.active_file_tabs_mut() {
                    t.next_tab();
                }
            }
            (_, KeyCode::BackTab) => {
                if let Some(t) = self.active_file_tabs_mut() {
                    t.prev_tab();
                }
            }
            (true, KeyCode::Char(c)) if c.is_ascii_digit() => {
                if let Some(d) = c.to_digit(10) {
                    if d >= 1 {
                        if let Some(t) = self.active_file_tabs_mut() {
                            t.select((d as usize) - 1);
                        }
                    }
                }
            }
            (true, KeyCode::Char('t')) => {
                if let Some(t) = self.active_file_tabs_mut() {
                    t.add_clone()?;
                }
            }
            (true, KeyCode::Char('w')) => {
                if let Some(t) = self.active_file_tabs_mut() {
                    t.close_active();
                }
            }
            // cursor
            (false, KeyCode::Char('j')) | (_, KeyCode::Down) => {
                if let Some(p) = self.active_pane_mut() {
                    p.move_cursor(1);
                }
            }
            (false, KeyCode::Char('k')) | (_, KeyCode::Up) => {
                if let Some(p) = self.active_pane_mut() {
                    p.move_cursor(-1);
                }
            }
            // navigation in/out
            (false, KeyCode::Char('h'))
            | (false, KeyCode::Char('-'))
            | (_, KeyCode::Left)
            | (_, KeyCode::Backspace) => {
                if let Some(p) = self.active_pane_mut() {
                    p.go_parent()?;
                }
            }
            (false, KeyCode::Char('l')) | (_, KeyCode::Right) => {
                if let Some(p) = self.active_pane_mut() {
                    let is_dir = p.selected().map(|e| e.is_dir).unwrap_or(false);
                    if is_dir {
                        p.enter_selected()?;
                    } else {
                        self.open_externally();
                    }
                }
            }
            // enter (file or dir): dir → enter, file → OS open (stub until sprint 5 viewer)
            (_, KeyCode::Enter) => {
                let is_dir = self
                    .active_pane()
                    .and_then(|p| p.selected())
                    .map(|e| e.is_dir)
                    .unwrap_or(false);
                if is_dir {
                    if let Some(p) = self.active_pane_mut() {
                        p.enter_selected()?;
                    }
                } else {
                    self.open_externally();
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn os_open(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(target_os = "linux")]
    let mut cmd = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.arg("/C").arg("start").arg("");
        c
    };

    cmd.arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

pub fn run(left: PathBuf, right: PathBuf) -> Result<()> {
    let mut app = App::new(left, right)?;
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, app))?;
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.handle_key(key)?;
                }
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let bottom_lines = if app.mode == Mode::Command { 2 } else { 1 };
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(bottom_lines)])
        .split(area);
    let main_area = vertical[0];
    let bottom_area = vertical[1];

    let main_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(main_area);
    let panes_area = main_split[0];
    let shell_area = main_split[1];

    let panes_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(panes_area);

    draw_file_pane(f, panes_split[0], &app.left, app.focused == FocusedPane::Left);
    draw_file_pane(f, panes_split[1], &app.right, app.focused == FocusedPane::Right);
    draw_shell_placeholder(f, shell_area, &app.shell, app.focused == FocusedPane::Shell);

    if app.mode == Mode::Command {
        let cmd_area = Rect::new(bottom_area.x, bottom_area.y, bottom_area.width, 1);
        let status_area = Rect::new(bottom_area.x, bottom_area.y + 1, bottom_area.width, 1);
        draw_command_line(f, cmd_area, &app.command_buffer);
        draw_status(f, status_area, app);
    } else {
        draw_status(f, bottom_area, app);
    }
}

fn tabs_title<'a>(tabs: &'a PaneTabs, focused: bool) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::with_capacity(tabs.tabs.len() * 4);
    spans.push(Span::raw(" "));
    for (i, tab) in tabs.tabs.iter().enumerate() {
        let label = format!(" {} {} ", i + 1, short_name(&tab.cwd));
        let style = if i == tabs.active {
            if focused {
                Style::default()
                    .fg(Color::Black)
                    .bg(CIAN_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White).bg(Color::DarkGray)
            }
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(label, style));
        if i + 1 < tabs.tabs.len() {
            spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
        }
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn shell_tabs_title<'a>(tabs: &'a ShellTabs, focused: bool) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    spans.push(Span::raw(" "));
    for i in 0..tabs.count {
        let label = format!(" shell {} ", i + 1);
        let style = if i == tabs.active {
            if focused {
                Style::default()
                    .fg(Color::Black)
                    .bg(CIAN_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White).bg(Color::DarkGray)
            }
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(label, style));
        if i + 1 < tabs.count {
            spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
        }
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn short_name(p: &Path) -> String {
    p.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            // root or unnamed → show short display
            let d = p.display().to_string();
            if d.is_empty() { "/".to_string() } else { d }
        })
}

fn draw_file_pane(f: &mut Frame, area: Rect, tabs: &PaneTabs, focused: bool) {
    let border_style = if focused {
        Style::default().fg(CIAN_ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(tabs_title(tabs, focused));

    let pane = tabs.active_ref();
    let items: Vec<ListItem> = pane
        .entries
        .iter()
        .map(|e| {
            let prefix = if e.is_dir { "▸ " } else { "  " };
            let style = if e.is_dir {
                Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(e.name.clone(), style),
            ]))
        })
        .collect();

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(SELECTED_BG)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    if !pane.entries.is_empty() {
        state.select(Some(pane.cursor));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_shell_placeholder(f: &mut Frame, area: Rect, tabs: &ShellTabs, focused: bool) {
    let border_style = if focused {
        Style::default().fg(CIAN_ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(shell_tabs_title(tabs, focused));
    let body = if focused {
        "(shell focus) — real PTY arrives in sprint 4. Esc / Ctrl-k to return."
    } else {
        "shell pane — Ctrl-j to focus, :shell command, real PTY coming in sprint 4."
    };
    let p = Paragraph::new(body).block(block);
    f.render_widget(p, area);
}

fn draw_command_line(f: &mut Frame, area: Rect, buf: &str) {
    let text = format!(":{}", buf);
    let p = Paragraph::new(text).style(
        Style::default()
            .bg(Color::Rgb(20, 20, 30))
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(p, area);
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let mode_label = format!("[{}]", app.mode.short());
    let (path_str, item_count, sel_name) = match app.active_pane() {
        Some(p) => (
            p.cwd.display().to_string(),
            p.entries.len(),
            p.selected().map(|e| e.name.clone()).unwrap_or_default(),
        ),
        None => ("(shell)".to_string(), 0, String::new()),
    };
    let focus_label = match app.focused {
        FocusedPane::Left => "L",
        FocusedPane::Right => "R",
        FocusedPane::Shell => "S",
    };
    // marks not yet implemented; reserve display
    let mark_count = 0usize;
    let msg = app.message.clone().unwrap_or_default();
    let text = format!(
        " {} {}  ·  {}  ·  {} items  ·  mask:{}  ·  marks:{}  ·  {}  {}",
        mode_label, focus_label, path_str, item_count, app.mask, mark_count, sel_name, msg,
    );
    let p = Paragraph::new(text).style(Style::default().bg(STATUS_BG).fg(Color::White));
    f.render_widget(p, area);
}
