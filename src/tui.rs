use std::{error::Error, io, path::PathBuf, process::Command, env};
use std::io::Write as IoWrite;
use tempfile::NamedTempFile;
use serde::Serialize;
use serde_yaml;
use std::time::{Instant, Duration};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind, MouseButton},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Clear},
    Frame, Terminal,
};
use twk::wiki::{Wiki, Information};
use twk::helpers::Locked;
use uuid::Uuid;
use regex::Regex;
use nucleo_matcher::{Config, Matcher, Utf32String};

#[derive(PartialEq, Eq)]
enum InputMode {
    Normal,
    Command,
    Edit,
}

pub struct App {
    wiki: Wiki,
    items: Vec<(String, String, Vec<String>, Uuid, PathBuf)>, // Name, Preview, Tags, ID, Path
    state: ListState,
    input_mode: InputMode,
    input: String,
    should_quit: bool,
    status_msg: String,
    status_timer: Option<Instant>,
    status_duration: Duration,
    use_global: bool,
    history: Vec<String>,
    history_pos: Option<usize>,
    filter: Option<String>,
    filter_regex: Option<Regex>,
    show_help: bool,
    // Inline edit state
    edit_buffer: String,
    editing_id: Option<Uuid>,
}

impl App {
    pub fn new(wiki: Wiki, use_global: bool) -> App {
        let mut app = App {
            wiki,
            items: Vec::new(),
            state: ListState::default(),
            input_mode: InputMode::Normal,
            input: String::new(),
            should_quit: false,
            status_msg: String::new(),
            status_timer: None,
            status_duration: Duration::from_secs(3),
            use_global,
            history: Vec::new(),
            history_pos: None,
            filter: None,
            filter_regex: None,
            show_help: false,
            edit_buffer: String::new(),
            editing_id: None,
        };
        app.refresh_items();
        if !app.items.is_empty() {
            app.state.select(Some(0));
        }
        app
    }

    pub fn refresh_items(&mut self) {
        self.items.clear();
        for locked_info in &self.wiki.info {
            let info = locked_info.read();
            let preview = info.data.lines().next().unwrap_or("").to_string();
            let path = info.path(&self.wiki);
            self.items.push((info.name.clone(), preview, info.tags.clone(), info.id, path));
        }

        // Apply filter if present
        if let Some(pattern) = &self.filter {
            if let Some(re) = &self.filter_regex {
                self.items.retain(|(name, preview, tags, _id, _path)| {
                    re.is_match(name) || re.is_match(preview) || tags.iter().any(|t| re.is_match(t))
                });
            } else {
                // Use nucleo-matcher fuzzy scoring and sort by score
                let mut scored: Vec<(i64, (String, String, Vec<String>, Uuid, PathBuf))> = Vec::new();
                let mut matcher = Matcher::new(Config::DEFAULT);
                let needle = Utf32String::from(pattern.as_str());

                for tuple in self.items.drain(..) {
                    let name_h = Utf32String::from(tuple.0.as_str());
                    let preview_h = Utf32String::from(tuple.1.as_str());

                    let name_score = matcher.fuzzy_match(name_h.slice(..), needle.slice(..));
                    let preview_score = matcher.fuzzy_match(preview_h.slice(..), needle.slice(..));

                    if let Some(score) = name_score.or(preview_score) {
                        scored.push((score as i64, tuple));
                    }
                }

                // sort descending by score
                scored.sort_by(|a, b| b.0.cmp(&a.0));
                self.items = scored.into_iter().map(|(_, t)| t).collect();
            }
        }
    }

    pub fn next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => {
                if i >= self.items.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    pub fn previous(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    self.items.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    pub fn switch_wiki(&mut self, name: String) {
        self.wiki = Wiki::load_or_create(name, self.use_global);
        self.refresh_items();
        self.state.select(Some(0));
        self.set_status(format!("Switched to wiki: {}", self.wiki.name));
    }

    pub fn create_entry(&mut self, name: String) {
        let id = Uuid::new_v4();
        let info = Information {
            id,
            tags: Vec::new(),
            name: name.clone(),
            data: String::new(),
        };

        let path = info.path(&self.wiki);
        if let Ok(locked) = Locked::new(path, info) {
            self.wiki.info.push(locked);
            self.refresh_items();
            self.set_status(format!("Created entry: {}", name));
        } else {
            self.set_status(format!("Failed to create entry: {}", name));
        }
    }

    fn find_locked_index_by_id(&self, id: Uuid) -> Option<usize> {
        for (i, locked) in self.wiki.info.iter().enumerate() {
            if locked.read().id == id {
                return Some(i);
            }
        }
        None
    }

    pub fn start_inline_edit(&mut self) {
        if let Some(sel) = self.state.selected() {
            if sel < self.items.len() {
                let id = self.items[sel].3;
                if let Some(li) = self.find_locked_index_by_id(id) {
                    let info = self.wiki.info[li].read();
                    let name_clone = info.name.clone();
                    self.edit_buffer = info.data.clone();
                    drop(info);
                    self.editing_id = Some(id);
                    self.input_mode = InputMode::Edit;
                    self.set_status(format!("Editing: {}", name_clone));
                }
            }
        }
    }

    pub fn save_inline_edit(&mut self) {
        if let Some(edit_id) = self.editing_id {
            if let Some(li) = self.find_locked_index_by_id(edit_id) {
                if let Some(locked) = self.wiki.info.get(li) {
                    let mut w = locked.write();
                    w.data = self.edit_buffer.clone();
                }
                self.refresh_items();
                self.input_mode = InputMode::Normal;
                self.editing_id = None;
                self.set_status("Saved.".to_string());
            }
        }
    }

    pub fn cancel_inline_edit(&mut self) {
        self.editing_id = None;
        self.edit_buffer.clear();
        self.input_mode = InputMode::Normal;
        self.set_status("Edit cancelled.".to_string());
    }
}

impl App {
    fn set_status(&mut self, s: String) {
        self.status_msg = s;
        self.status_timer = Some(Instant::now());
    }
}

pub fn run(wiki_name: String, use_global: bool) -> Result<(), Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let wiki = Wiki::load_or_create(wiki_name, use_global);
    let mut app = App::new(wiki, use_global);

    let res = run_app(&mut terminal, &mut app);

    // restore terminal on exit
    disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err)
    }

    Ok(())
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        let event = event::read()?;
        match event {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Press {
                    // If help overlay is visible, allow a small set of keys to close it
                    if app.show_help {
                        match key.code {
                            KeyCode::F(1) | KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('h') => {
                                app.show_help = false;
                                continue;
                            }
                            _ => {
                                // ignore other keys while help is shown
                                continue;
                            }
                        }
                    }

                    match app.input_mode {
                        InputMode::Normal => match key.code {
                            KeyCode::Char('q') => app.should_quit = true,
                            KeyCode::Char(':') => {
                                app.input_mode = InputMode::Command;
                                app.input.push(':');
                            }
                            KeyCode::Char('j') | KeyCode::Down => app.next(),
                            KeyCode::Char('k') | KeyCode::Up => app.previous(),
                            KeyCode::Char('i') => app.start_inline_edit(),
                            KeyCode::Enter | KeyCode::Char('e') => {
                                // Open selected entry in external editor; pipe TITLE\n---\nCONTENT into a temp file,
                                // re-load the file after editor exits, and force a full redraw.
                                if let Some(idx) = app.state.selected() {
                                    if idx < app.items.len() {
                                        // get the id and clone current full content safely
                                        let id = app.items[idx].3;
                                        let mut name = String::new();
                                        let mut data = String::new();
                                        let mut tags: Vec<String> = Vec::new();
                                        if let Some(li) = app.find_locked_index_by_id(id) {
                                            let info = app.wiki.info[li].read();
                                            name = info.name.clone();
                                            data = info.data.clone();
                                            tags = info.tags.clone();
                                            drop(info);
                                        }

                                        // write to temp file with YAML frontmatter:
                                        // ---
                                        // title: ...
                                        // tags: [..]
                                        // ---
                                        // CONTENT
                                        #[derive(Serialize)]
                                        struct Front<'a> {
                                            title: &'a str,
                                            tags: &'a Vec<String>,
                                        }

                                        let mut tmp = match NamedTempFile::new() {
                                            Ok(t) => t,
                                            Err(_) => return Ok(()),
                                        };
                                        let fm = serde_yaml::to_string(&Front { title: &name, tags: &tags }).unwrap_or_default();
                                        let payload = format!("---\n{}---\n\n{}", fm, data);
                                        let _ = tmp.write_all(payload.as_bytes());
                                        let tmp_path = tmp.path().to_owned();

                                        // restore terminal
                                        disable_raw_mode()?;
                                        let mut stdout = io::stdout();
                                        execute!(stdout, LeaveAlternateScreen, DisableMouseCapture)?;

                                        // launch editor on the temp file
                                        let editor = env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
                                        let _ = Command::new(editor).arg(&tmp_path).status();

                                        // read edited contents back
                                        let edited = std::fs::read_to_string(&tmp_path).unwrap_or_default();
                                        // parse YAML frontmatter if present
                                        let mut new_title = String::new();
                                        let mut new_tags: Option<Vec<String>> = None;
                                        let mut rest = String::new();
                                        let cursor = edited.as_str();
                                        if cursor.trim_start().starts_with("---") {
                                            // find the frontmatter block
                                            if let Some(pos) = cursor.find("\n---") {
                                                let fm_block = &cursor[4..pos+1];
                                                // parse YAML
                                                if let Ok(fm_val) = serde_yaml::from_str::<serde_yaml::Value>(fm_block) {
                                                    if let Some(t) = fm_val.get("title") {
                                                        if let Some(s) = t.as_str() { new_title = s.to_string(); }
                                                    }
                                                    if let Some(tg) = fm_val.get("tags") {
                                                        if let Some(arr) = tg.as_sequence() {
                                                            let parsed: Vec<String> = arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
                                                            new_tags = Some(parsed);
                                                        }
                                                    }
                                                }
                                                // remainder after the closing '---' (skip the newline)
                                                let rest_start = pos + 5; // skip '\n---' and following newline
                                                if rest_start < cursor.len() {
                                                    rest = cursor[rest_start..].trim_start_matches('\n').to_string();
                                                }
                                            } else {
                                                // no closing delimiter; treat whole as content
                                                rest = edited;
                                            }
                                        } else {
                                            // fallback: first line title, optional '---' separator
                                            let mut lines = edited.lines();
                                            new_title = lines.next().unwrap_or("").to_string();
                                            let second = lines.next();
                                            if second == Some("---") {
                                                rest = lines.collect::<Vec<_>>().join("\n");
                                            } else {
                                                let mut v = Vec::new();
                                                if let Some(s) = second { v.push(s); }
                                                v.extend(lines);
                                                rest = v.join("\n");
                                            }
                                        }

                                        // re-enter tui
                                        let mut stdout = io::stdout();
                                        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
                                        enable_raw_mode()?;

                                        // write back into wiki (find it again to avoid stale refs)
                                        if let Some(li) = app.find_locked_index_by_id(id) {
                                            if let Some(locked) = app.wiki.info.get(li) {
                                                let mut w = locked.write();
                                                if !new_title.trim().is_empty() {
                                                    w.name = new_title.trim().to_string();
                                                }
                                                if let Some(ntags) = new_tags {
                                                    w.tags = ntags;
                                                }
                                                w.data = rest;
                                            }
                                        }

                                        // refresh items, force a clear draw so UI fully redraws
                                        app.refresh_items();
                                        let _ = terminal.draw(|f| f.render_widget(Clear, f.area()));
                                        app.set_status("Saved from editor".to_string());
                                    }
                                }
                            }
                            KeyCode::F(1) => app.show_help = !app.show_help,
                            _ => {}
                        },
                        InputMode::Command => match key.code {
                            KeyCode::Enter => {
                                let input: String = app.input.drain(..).collect();
                                // record history
                                if !input.trim().is_empty() {
                                    app.history.push(input.clone());
                                }
                                app.history_pos = None;
                                process_command(app, &input);
                                app.input_mode = InputMode::Normal;
                            }
                            KeyCode::Char(c) => {
                                app.input.push(c);
                                app.history_pos = None;
                            }
                            KeyCode::Up => {
                                // navigate history backwards
                                if !app.history.is_empty() {
                                    match app.history_pos {
                                        Some(0) => {}
                                        Some(n) => {
                                            let new = n - 1;
                                            app.history_pos = Some(new);
                                            app.input = app.history[new].clone();
                                        }
                                        None => {
                                            let last = app.history.len() - 1;
                                            app.history_pos = Some(last);
                                            app.input = app.history[last].clone();
                                        }
                                    }
                                }
                            }
                            KeyCode::Down => {
                                if !app.history.is_empty() {
                                    match app.history_pos {
                                        None => {}
                                        Some(n) => {
                                            if n + 1 < app.history.len() {
                                                let new = n + 1;
                                                app.history_pos = Some(new);
                                                app.input = app.history[new].clone();
                                            } else {
                                                app.history_pos = None;
                                                app.input.clear();
                                            }
                                        }
                                    }
                                }
                            }
                            KeyCode::Backspace => {
                                app.input.pop();
                                app.history_pos = None;
                                if app.input.is_empty() {
                                    app.input_mode = InputMode::Normal;
                                }
                            }
                            KeyCode::Esc => {
                                app.input.clear();
                                app.history_pos = None;
                                app.input_mode = InputMode::Normal;
                            }
                            _ => {}
                        },
                        InputMode::Edit => match key.code {
                            KeyCode::Enter => {
                                app.edit_buffer.push('\n');
                            }
                            KeyCode::Char(c) => {
                                // handle ctrl-s separately
                                if key.modifiers.contains(KeyModifiers::CONTROL) && c == 's' {
                                    app.save_inline_edit();
                                } else {
                                    app.edit_buffer.push(c);
                                }
                            }
                            KeyCode::Backspace => {
                                app.edit_buffer.pop();
                            }
                            KeyCode::Esc => {
                                app.cancel_inline_edit();
                            }
                            _ => {}
                        },
                    }
                }
            }
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollDown => app.next(),
                    MouseEventKind::ScrollUp => app.previous(),
                    MouseEventKind::Down(MouseButton::Left) => {
                        // Map mouse position to list index
                        if let Ok((cols, rows)) = crossterm::terminal::size() {
                            let area = Rect::new(0, 0, cols, rows);
                            let chunks = Layout::default()
                                .direction(Direction::Vertical)
                                .constraints([Constraint::Min(1), Constraint::Length(1)])
                                .split(area);
                            let list_area = chunks[0];
                            if mouse.row >= list_area.y && mouse.row < list_area.y + list_area.height {
                                let idx = (mouse.row - list_area.y) as usize;
                                if idx < app.items.len() {
                                    app.state.select(Some(idx));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn process_command(app: &mut App, command: &str) {
    let parts: Vec<&str> = command.trim_start_matches(':').split_whitespace().collect();
    if parts.is_empty() {
        return;
    }
    match parts[0] {
        "q" | "quit" => app.should_quit = true,
        "wiki" | "switch" => {
            if parts.len() > 1 {
                app.switch_wiki(parts[1].to_string());
            } else {
                app.status_msg = "Usage: :wiki <wiki_name>".to_string();
            }
        }
        "n" | "new" => {
            if parts.len() > 1 {
                app.create_entry(parts[1..].join(" "));
            } else {
                app.status_msg = "Usage: :n <entry_name>".to_string();
            }
        }
        "s" | "search" => {
            if parts.len() > 1 {
                let pat = parts[1..].join(" ");
                if pat.starts_with("re:") {
                    let raw = pat.trim_start_matches("re:");
                    match Regex::new(raw) {
                        Ok(r) => {
                            app.filter = Some(raw.to_string());
                            app.filter_regex = Some(r);
                            app.refresh_items();
                        }
                        Err(e) => app.status_msg = format!("Invalid regex: {}", e),
                    }
                } else {
                    app.filter = Some(pat.clone());
                    app.filter_regex = None;
                    app.refresh_items();
                }
            } else {
                // clear filter
                app.filter = None;
                app.filter_regex = None;
                app.refresh_items();
            }
        }
        "edit" => {
            app.start_inline_edit();
        }
        "help" | "?" => {
            app.show_help = !app.show_help;
        }
        _ => {
            app.status_msg = format!("Unknown command: {}", parts[0]);
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
            ])
        .split(f.area());

    let area = f.area();
    let width = area.width as usize;

    // Prepare tag displays and compute max tag width so the ' | ' separator aligns
    let tags_strs: Vec<String> = app
        .items
        .iter()
        .map(|(_, _, tags, _, _)| {
            if tags.is_empty() {
                "".to_string()
            } else {
                format!("[{}] ", tags.join(", "))
            }
        })
        .collect();

    let tags_max = tags_strs.iter().map(|s| s.chars().count()).max().unwrap_or(0);
    let title_max = std::cmp::max(10, if width > tags_max + 3 { (width - tags_max - 3) / 2 } else { 10 });

    let items: Vec<ListItem> = app
        .items
        .iter()
        .enumerate()
        .map(|(i, (name, preview, _tags, _id, _path))| {
            let mut title = name.clone();
            if title.chars().count() > title_max {
                title = title.chars().take(title_max - 1).collect::<String>() + "…";
            }
            let tags_display = &tags_strs[i];

            // compose combined left column with fixed width = tags_max + title_max
            let left = format!("{:tags_max$}{:title_max$}", tags_display, title, tags_max = tags_max, title_max = title_max);

            let content = Line::from(vec![
                Span::styled(left, Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" | "),
                Span::raw(preview),
            ]);
            ListItem::new(content)
        })
        .collect();

    let items = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(format!("Wiki: {}", app.wiki.name)))
        .highlight_style(
            Style::default()
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    f.render_stateful_widget(items, chunks[0], &mut app.state);

    // Command/status bar: show while in command mode or when a transient status is set
    let show_bar = app.input_mode == InputMode::Command
        || app.status_timer.map_or(false, |t| t.elapsed() < app.status_duration);

    if show_bar {
        let input_text = if app.input_mode == InputMode::Command {
            app.input.as_str()
        } else {
            app.status_msg.as_str()
        };
        f.render_widget(Clear, chunks[1]);
        let input = Paragraph::new(input_text)
            .style(Style::default().bg(Color::White).fg(Color::Black))
            .block(Block::default());
        f.render_widget(input, chunks[1]);
    }

    if app.input_mode == InputMode::Edit {
        // Render editor overlay
        let editor = Paragraph::new(app.edit_buffer.as_str())
            .block(Block::default().borders(Borders::ALL).title("Edit (Ctrl-S to save, Esc to cancel)"))
            .style(Style::default().fg(Color::White));
        let area = centered_rect(80, 60, f.area());
        f.render_widget(Clear, area);
        f.render_widget(editor, area);
    }

    if app.show_help {
        let help_text = "Navigation: j/k or ↑/↓ • Click to select
: (colon) enter command mode
Commands: :n <name> (new), :wiki <name> (switch), :s <query> (fuzzy), :s re:<regex> (regex), :edit (inline), :q quit
Keys: i edit inline, e/Enter external editor, F1 or :help show this help";
        let help = Paragraph::new(help_text).block(Block::default().borders(Borders::ALL).title("Help"));
        let area = centered_rect(60, 40, f.area());
        f.render_widget(Clear, area);
        f.render_widget(help, area);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    let vertical = popup_layout[1];

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical);

    horizontal[1]
}

