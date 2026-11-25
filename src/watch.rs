//! Real-time "source control" view with boxes flush to the left:
//! a file list (Changes / Files explorer) next to a code viewer that shows
//! git additions in green and removals in red. Keyboard and mouse supported.
//!
//! Architecture: all git work happens on a background worker thread. The UI
//! thread only draws and handles input, so it never blocks on a git process.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::style::{Color, Stylize};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};

use super::GitRepo;

const INPUT_POLL: Duration = Duration::from_millis(25);
const STATUS_REFRESH: Duration = Duration::from_millis(1000);
const SYNC_REFRESH: Duration = Duration::from_secs(3);
const MESSAGE_TTL: Duration = Duration::from_secs(8);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const MAX_VIEW_LINES: usize = 20_000;
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const WHEEL_BASE_STEP: usize = 2;
const WHEEL_BURST: Duration = Duration::from_millis(70);
const WHEEL_MAX_BOOST: usize = 8;

const C_BORDER: Color = Color::Rgb { r: 90, g: 96, b: 110 };
const C_ACCENT: Color = Color::Rgb { r: 90, g: 200, b: 250 };
const C_DIM: Color = Color::Rgb { r: 120, g: 126, b: 140 };
const C_ADD_FG: Color = Color::Rgb { r: 130, g: 225, b: 150 };
const C_ADD_BG: Color = Color::Rgb { r: 18, g: 52, b: 30 };
const C_DEL_FG: Color = Color::Rgb { r: 245, g: 135, b: 135 };
const C_DEL_BG: Color = Color::Rgb { r: 62, g: 24, b: 24 };
const C_SEL_BG: Color = Color::Rgb { r: 44, g: 52, b: 78 };
const C_MOD: Color = Color::Rgb { r: 235, g: 200, b: 100 };
const C_REN: Color = Color::Rgb { r: 100, g: 210, b: 220 };
const C_DIR: Color = Color::Rgb { r: 120, g: 165, b: 255 };

// ============================================================================
// DATA
// ============================================================================

#[derive(Debug, PartialEq, Clone)]
struct Entry {
    code: String,
    path: String,
}

#[derive(Clone, Copy)]
enum Level {
    Info,
    Ok,
    Err,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Changes,
    Files,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum LineKind {
    Add,
    Del,
    Ctx,
    Note,
}

#[derive(Debug, PartialEq)]
struct ViewLine {
    kind: LineKind,
    old_no: Option<usize>,
    new_no: Option<usize>,
    text: String,
}

impl ViewLine {
    fn note(text: &str) -> Self {
        ViewLine {
            kind: LineKind::Note,
            old_no: None,
            new_no: None,
            text: text.to_string(),
        }
    }
}

struct TreeRow {
    depth: usize,
    name: String,
    path: String,
    is_dir: bool,
    collapsed: bool,
}

/// UI -> worker.
enum Cmd {
    /// Which file the viewer should show (None = nothing / a folder).
    View(Option<String>),
    /// Whether the Files explorer is visible (the file list is only loaded then).
    WantFiles(bool),
    Commit { message: String, push: bool },
    Push,
    Pull,
}

/// Worker -> UI.
enum Update {
    Status(Vec<Entry>),
    Sync {
        branch: String,
        ahead: usize,
        behind: usize,
        has_remote: bool,
    },
    Files(Vec<String>),
    View {
        path: String,
        lines: Vec<ViewLine>,
    },
    Done {
        level: Level,
        text: String,
        clear_message: bool,
    },
}

/// Screen geometry, shared by rendering and mouse hit-testing.
#[derive(Clone, Copy)]
struct Layout {
    width: usize,
    height: usize,
    left_w: usize,
    right_w: usize,
    box_h: usize,
    tab_changes: (usize, usize),
    tab_files: (usize, usize),
}

fn compute_layout(width: usize, height: usize, n_changes: usize) -> Option<Layout> {
    if width < 60 || height < 12 {
        return None;
    }
    let left_w = (width * 32 / 100).clamp(30, 52);
    let label = format!("Changes ({})", n_changes).chars().count();
    Some(Layout {
        width,
        height,
        left_w,
        right_w: width - left_w,
        box_h: height - 5,
        tab_changes: (3, 3 + label),
        tab_files: (3 + label + 3, 3 + label + 3 + 5),
    })
}

struct App {
    repo_name: String,
    tx: Sender<Cmd>,
    updates: Receiver<Update>,
    mode: Mode,
    loaded: bool,
    busy: Option<String>,
    entries: Vec<Entry>,
    files: Vec<String>,
    collapsed: HashSet<String>,
    rows: Vec<TreeRow>,
    sel_change: Option<String>,
    sel_file: Option<String>,
    scroll_changes: usize,
    scroll_files: usize,
    follow: bool,
    view: Vec<ViewLine>,
    view_path: Option<String>,
    requested_view: Option<String>,
    view_scroll: usize,
    layout: Option<Layout>,
    branch: String,
    ahead: usize,
    behind: usize,
    has_remote: bool,
    message: String,
    status: Option<(Level, String, Instant)>,
    started: Instant,
    last_wheel: Option<Instant>,
    wheel_streak: usize,
}

/// Restores the terminal even if we exit through an error or a panic.
struct TermGuard;

impl TermGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        Ok(TermGuard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture, Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

// ============================================================================
// GIT HELPERS (silent: nothing is printed, output is returned)
// ============================================================================

fn git(repo: &GitRepo, args: &[&str]) -> Result<String, String> {
    let mut cmd = repo.create_command(args);
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Err(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(stderr)
        }
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("unknown error")
        .to_string()
}

/// Parses one `git status --porcelain=v1 -z` record ("XY path").
fn parse_entry(record: &str) -> Option<Entry> {
    let code = record.get(..2)?.trim().to_string();
    let path = record.get(3..)?.trim().to_string();
    if code.is_empty() || path.is_empty() {
        return None;
    }
    Some(Entry { code, path })
}

fn load_entries(repo: &GitRepo) -> Option<Vec<Entry>> {
    let out = git(repo, &["status", "--porcelain=v1", "-z", "-uall"]).ok()?;
    let mut records = out.split('\0').filter(|r| !r.is_empty());
    let mut entries = Vec::new();
    while let Some(record) = records.next() {
        if let Some(entry) = parse_entry(record) {
            let renamed = entry.code.contains('R') || entry.code.contains('C');
            entries.push(entry);
            if renamed {
                // In -z mode a rename is followed by the original path.
                records.next();
            }
        }
    }
    Some(entries)
}

fn load_files(repo: &GitRepo) -> Option<Vec<String>> {
    let out = git(
        repo,
        &["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
    )
    .ok()?;
    let mut files: Vec<String> = out
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect();
    files.sort();
    files.dedup();
    Some(files)
}

fn parse_counts(text: &str) -> Option<(usize, usize)> {
    let mut parts = text.split_whitespace();
    let ahead = parts.next()?.parse().ok()?;
    let behind = parts.next()?.parse().ok()?;
    Some((ahead, behind))
}

/// Branch, ahead/behind and remote presence, without printing anything.
fn read_sync(repo: &GitRepo) -> Update {
    let branch = git(repo, &["symbolic-ref", "--short", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "(detached)".to_string());
    let (ahead, behind) = git(repo, &["rev-list", "--left-right", "--count", "HEAD...@{u}"])
        .ok()
        .and_then(|s| parse_counts(&s))
        .unwrap_or((0, 0));
    let has_remote = git(repo, &["remote"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    Update::Sync {
        branch,
        ahead,
        behind,
        has_remote,
    }
}

fn clean_text(text: &str) -> String {
    text.replace('\r', "").replace('\t', "    ")
}

fn parse_hunk_start(token: &str) -> usize {
    token
        .trim_start_matches(|c| c == '-' || c == '+')
        .split(',')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(1)
}

/// Turns a unified diff (with huge context) into annotated lines.
fn parse_diff(diff: &str) -> Vec<ViewLine> {
    let mut out: Vec<ViewLine> = Vec::new();
    let (mut old_no, mut new_no) = (1usize, 1usize);
    let mut in_hunk = false;

    for line in diff.lines() {
        if line.starts_with("@@") {
            let mut parts = line.split_whitespace();
            parts.next();
            old_no = parse_hunk_start(parts.next().unwrap_or("-1"));
            new_no = parse_hunk_start(parts.next().unwrap_or("+1"));
            if in_hunk {
                out.push(ViewLine::note("···"));
            }
            in_hunk = true;
            continue;
        }
        if !in_hunk || line.starts_with('\\') {
            continue;
        }

        let (kind, text) = match line.chars().next() {
            Some('+') => (LineKind::Add, &line[1..]),
            Some('-') => (LineKind::Del, &line[1..]),
            Some(' ') => (LineKind::Ctx, &line[1..]),
            _ => (LineKind::Ctx, line),
        };

        let (o, n) = match kind {
            LineKind::Add => (None, Some(new_no)),
            LineKind::Del => (Some(old_no), None),
            _ => (Some(old_no), Some(new_no)),
        };
        if kind != LineKind::Add {
            old_no += 1;
        }
        if kind != LineKind::Del {
            new_no += 1;
        }

        out.push(ViewLine {
            kind,
            old_no: o,
            new_no: n,
            text: clean_text(text),
        });
        if out.len() >= MAX_VIEW_LINES {
            break;
        }
    }
    out
}

fn read_file_lines(repo: &GitRepo, path: &str, kind: LineKind) -> Vec<ViewLine> {
    let bytes = match fs::read(repo.root.join(path)) {
        Ok(b) => b,
        Err(_) => return vec![ViewLine::note("Cannot read this file (deleted or not a file)")],
    };
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return vec![ViewLine::note("Binary file")];
    }
    if bytes.len() > MAX_FILE_BYTES {
        return vec![ViewLine::note("File too large to preview")];
    }
    String::from_utf8_lossy(&bytes)
        .lines()
        .take(MAX_VIEW_LINES)
        .enumerate()
        .map(|(i, text)| ViewLine {
            kind,
            old_no: None,
            new_no: Some(i + 1),
            text: clean_text(text),
        })
        .collect()
}

fn build_view(repo: &GitRepo, path: &str, code: Option<&str>) -> Vec<ViewLine> {
    match code {
        Some(c) if c.contains('?') => read_file_lines(repo, path, LineKind::Add),
        Some(_) => {
            match git(
                repo,
                &["diff", "HEAD", "-U99999", "--no-color", "--no-ext-diff", "--", path],
            ) {
                Ok(diff) => {
                    let lines = parse_diff(&diff);
                    if lines.is_empty() {
                        read_file_lines(repo, path, LineKind::Ctx)
                    } else {
                        lines
                    }
                }
                // No HEAD yet (first commit): everything is new.
                Err(_) => read_file_lines(repo, path, LineKind::Add),
            }
        }
        None => read_file_lines(repo, path, LineKind::Ctx),
    }
}

/// Sort key that lists folders before files at every level (like VS Code).
fn sort_key(path: &str) -> Vec<(bool, String)> {
    let parts: Vec<&str> = path.split('/').collect();
    let last = parts.len() - 1;
    parts
        .iter()
        .enumerate()
        .map(|(i, s)| (i == last, s.to_lowercase()))
        .collect()
}

fn status_letter(code: &str) -> &'static str {
    if code.contains('?') {
        return "U";
    }
    match code.chars().find(|c| *c != ' ') {
        Some('A') => "A",
        Some('D') => "D",
        Some('R') => "R",
        Some('C') => "C",
        _ => "M",
    }
}

fn status_color(code: &str) -> Color {
    match status_letter(code) {
        "U" | "A" => C_ADD_FG,
        "D" => C_DEL_FG,
        "R" | "C" => C_REN,
        _ => C_MOD,
    }
}

// ============================================================================
// WORKER THREAD (all git processes run here)
// ============================================================================

fn do_push(repo: &GitRepo) -> (Level, String) {
    let remote = git(repo, &["remote"]).map(|s| !s.trim().is_empty()).unwrap_or(false);
    if !remote {
        return (Level::Err, "No remote configured".to_string());
    }
    let branch = git(repo, &["symbolic-ref", "--short", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "main".to_string());
    let has_upstream = git(repo, &["rev-parse", "--abbrev-ref", "@{u}"]).is_ok();
    let args: Vec<&str> = if has_upstream {
        vec!["push"]
    } else {
        vec!["push", "-u", "origin", branch.as_str()]
    };
    match git(repo, &args) {
        Ok(_) => (Level::Ok, "Pushed successfully".to_string()),
        Err(e) => (
            Level::Err,
            format!("Push failed (commit is saved locally): {}", first_line(&e)),
        ),
    }
}

fn do_pull(repo: &GitRepo) -> (Level, String) {
    match git(repo, &["pull", "--rebase", "--autostash"]) {
        Ok(_) => (Level::Ok, "Up to date with remote".to_string()),
        Err(e) => (Level::Err, format!("Pull failed: {}", first_line(&e))),
    }
}

/// Returns (level, text, commit_succeeded).
fn do_commit(repo: &GitRepo, message: &str, push: bool) -> (Level, String, bool) {
    if let Err(e) = git(repo, &["add", "-A"]) {
        return (Level::Err, format!("git add failed: {}", first_line(&e)), false);
    }
    if let Err(e) = git(repo, &["commit", "-m", message]) {
        return (Level::Err, format!("Commit failed: {}", first_line(&e)), false);
    }
    if !push {
        return (Level::Ok, "Committed".to_string(), true);
    }
    let remote = git(repo, &["remote"]).map(|s| !s.trim().is_empty()).unwrap_or(false);
    if !remote {
        return (
            Level::Ok,
            "Committed (no remote configured, nothing to push)".to_string(),
            true,
        );
    }
    let (level, text) = do_push(repo);
    (level, text, true)
}

fn worker(repo: GitRepo, commands: Receiver<Cmd>, updates: Sender<Update>) {
    let mut entries: Vec<Entry> = Vec::new();
    let mut have_entries = false;
    let mut want_files = false;
    let mut view_path: Option<String> = None;
    let mut view_stamp: Option<SystemTime> = None;
    let mut next_status = Instant::now();
    let mut next_sync = Instant::now();
    let mut force_status = true;
    let mut force_sync = true;
    let mut force_view = false;

    loop {
        let wait = next_status
            .min(next_sync)
            .saturating_duration_since(Instant::now());

        match commands.recv_timeout(wait) {
            Ok(Cmd::View(path)) => {
                view_path = path;
                force_view = true;
            }
            Ok(Cmd::WantFiles(wanted)) => {
                want_files = wanted;
                if wanted {
                    force_sync = true;
                }
            }
            Ok(Cmd::Commit { message, push }) => {
                let (level, text, clear_message) = do_commit(&repo, &message, push);
                let _ = updates.send(Update::Done {
                    level,
                    text,
                    clear_message,
                });
                force_status = true;
                force_sync = true;
            }
            Ok(Cmd::Push) => {
                let (level, text) = do_push(&repo);
                let _ = updates.send(Update::Done {
                    level,
                    text,
                    clear_message: false,
                });
                force_status = true;
                force_sync = true;
            }
            Ok(Cmd::Pull) => {
                let (level, text) = do_pull(&repo);
                let _ = updates.send(Update::Done {
                    level,
                    text,
                    clear_message: false,
                });
                force_status = true;
                force_sync = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        let now = Instant::now();

        if force_status || now >= next_status {
            if let Some(fresh) = load_entries(&repo) {
                if !have_entries || fresh != entries {
                    entries = fresh;
                    have_entries = true;
                    force_view = true;
                    if updates.send(Update::Status(entries.clone())).is_err() {
                        return;
                    }
                }
            }
            next_status = Instant::now() + STATUS_REFRESH;
            force_status = false;
        }

        if force_sync || now >= next_sync {
            if updates.send(read_sync(&repo)).is_err() {
                return;
            }
            if want_files {
                if let Some(files) = load_files(&repo) {
                    if updates.send(Update::Files(files)).is_err() {
                        return;
                    }
                }
            }
            next_sync = Instant::now() + SYNC_REFRESH;
            force_sync = false;
        }

        // Only rebuild the viewer when the selection, the git status or the
        // file itself changed (modification time) since the last time.
        match &view_path {
            Some(path) => {
                let stamp = fs::metadata(repo.root.join(path))
                    .and_then(|m| m.modified())
                    .ok();
                if force_view || stamp != view_stamp {
                    let code = entries
                        .iter()
                        .find(|e| &e.path == path)
                        .map(|e| e.code.clone());
                    let lines = build_view(&repo, path, code.as_deref());
                    view_stamp = stamp;
                    force_view = false;
                    if updates
                        .send(Update::View {
                            path: path.clone(),
                            lines,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
            None => {
                force_view = false;
                view_stamp = None;
            }
        }
    }
}

// ============================================================================
// APP LOGIC (UI thread: never runs git)
// ============================================================================

impl App {
    fn new(repo: &GitRepo) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (up_tx, up_rx) = mpsc::channel();
        let worker_repo = GitRepo {
            root: repo.root.clone(),
            name: repo.name.clone(),
        };
        thread::spawn(move || worker(worker_repo, cmd_rx, up_tx));

        App {
            repo_name: repo.name.clone(),
            tx: cmd_tx,
            updates: up_rx,
            mode: Mode::Changes,
            loaded: false,
            busy: None,
            entries: Vec::new(),
            files: Vec::new(),
            collapsed: HashSet::new(),
            rows: Vec::new(),
            sel_change: None,
            sel_file: None,
            scroll_changes: 0,
            scroll_files: 0,
            follow: true,
            view: Vec::new(),
            view_path: None,
            requested_view: None,
            view_scroll: 0,
            layout: None,
            branch: String::new(),
            ahead: 0,
            behind: 0,
            has_remote: false,
            message: String::new(),
            status: None,
            started: Instant::now(),
            last_wheel: None,
            wheel_streak: 0,
        }
    }

    fn set(&mut self, level: Level, text: impl Into<String>) {
        self.status = Some((level, text.into(), Instant::now()));
    }

    /// True while something on screen moves by itself (spinners).
    fn animating(&self) -> bool {
        self.busy.is_some() || !self.loaded || (self.mode == Mode::Changes && self.entries.is_empty())
    }

    /// Applies everything the worker has produced. Returns true if the screen changed.
    fn drain_updates(&mut self) -> bool {
        let mut changed = false;
        while let Ok(update) = self.updates.try_recv() {
            changed = true;
            match update {
                Update::Status(entries) => {
                    self.entries = entries;
                    self.loaded = true;
                    self.normalize_selection();
                    self.sync_view_request();
                }
                Update::Sync {
                    branch,
                    ahead,
                    behind,
                    has_remote,
                } => {
                    self.branch = branch;
                    self.ahead = ahead;
                    self.behind = behind;
                    self.has_remote = has_remote;
                }
                Update::Files(files) => {
                    self.files = files;
                    self.rebuild_rows();
                    self.normalize_selection();
                    self.sync_view_request();
                }
                Update::View { path, lines } => {
                    if self.view_path.as_deref() == Some(path.as_str()) {
                        self.view = lines;
                    }
                }
                Update::Done {
                    level,
                    text,
                    clear_message,
                } => {
                    self.busy = None;
                    if clear_message {
                        self.message.clear();
                    }
                    self.set(level, text);
                }
            }
        }
        changed
    }

    // ---- selection -------------------------------------------------------

    fn list_len(&self) -> usize {
        match self.mode {
            Mode::Changes => self.entries.len(),
            Mode::Files => self.rows.len(),
        }
    }

    fn selected_path(&self) -> Option<String> {
        match self.mode {
            Mode::Changes => self.sel_change.clone(),
            Mode::Files => self.sel_file.clone(),
        }
    }

    fn current_index(&self) -> usize {
        let found = match self.mode {
            Mode::Changes => self
                .sel_change
                .as_ref()
                .and_then(|p| self.entries.iter().position(|e| &e.path == p)),
            Mode::Files => self
                .sel_file
                .as_ref()
                .and_then(|p| self.rows.iter().position(|r| &r.path == p)),
        };
        found.unwrap_or(0)
    }

    fn normalize_selection(&mut self) {
        let change_ok = self
            .sel_change
            .as_ref()
            .map_or(false, |p| self.entries.iter().any(|e| &e.path == p));
        if !change_ok {
            self.sel_change = self.entries.first().map(|e| e.path.clone());
        }
        let file_ok = self
            .sel_file
            .as_ref()
            .map_or(false, |p| self.rows.iter().any(|r| &r.path == p));
        if !file_ok {
            self.sel_file = self.rows.first().map(|r| r.path.clone());
        }
    }

    /// Tells the worker which file to show whenever the selection changed.
    fn sync_view_request(&mut self) {
        let path = self.selected_path();
        if path == self.requested_view {
            return;
        }
        self.requested_view = path.clone();
        self.view_path = path.clone();
        self.view_scroll = 0;

        match path {
            None => {
                self.view.clear();
                let _ = self.tx.send(Cmd::View(None));
            }
            Some(p) => {
                let is_dir = self.mode == Mode::Files
                    && self.rows.iter().any(|r| r.path == p && r.is_dir);
                if is_dir {
                    self.view = vec![ViewLine::note(
                        "Folder: use Right / Left (or click) to expand or collapse",
                    )];
                    let _ = self.tx.send(Cmd::View(None));
                } else {
                    let _ = self.tx.send(Cmd::View(Some(p)));
                }
            }
        }
    }

    fn select_index(&mut self, index: usize) {
        match self.mode {
            Mode::Changes => {
                if let Some(e) = self.entries.get(index) {
                    self.sel_change = Some(e.path.clone());
                }
            }
            Mode::Files => {
                if let Some(r) = self.rows.get(index) {
                    self.sel_file = Some(r.path.clone());
                }
            }
        }
        self.follow = true;
        self.sync_view_request();
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.list_len();
        if len == 0 {
            return;
        }
        let next = (self.current_index() as isize + delta).clamp(0, len as isize - 1);
        self.select_index(next as usize);
    }

    fn switch_mode(&mut self, mode: Mode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        let _ = self.tx.send(Cmd::WantFiles(mode == Mode::Files));
        if mode == Mode::Files {
            self.rebuild_rows();
        }
        self.normalize_selection();
        self.follow = true;
        // The same path may be selected in both modes; force a fresh request.
        self.requested_view = None;
        self.sync_view_request();
    }

    fn selected_is_dir(&self) -> Option<(String, bool)> {
        if self.mode != Mode::Files {
            return None;
        }
        let path = self.sel_file.as_ref()?;
        self.rows
            .iter()
            .find(|r| &r.path == path && r.is_dir)
            .map(|r| (r.path.clone(), r.collapsed))
    }

    fn set_collapsed(&mut self, path: &str, collapsed: bool) {
        if collapsed {
            self.collapsed.insert(path.to_string());
        } else {
            self.collapsed.remove(path);
        }
        self.rebuild_rows();
        self.normalize_selection();
        self.sync_view_request();
    }

    fn rebuild_rows(&mut self) {
        let mut sorted: Vec<&String> = self.files.iter().collect();
        sorted.sort_by_cached_key(|p| sort_key(p));

        let mut rows: Vec<TreeRow> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        'files: for path in sorted {
            let parts: Vec<&str> = path.split('/').collect();
            for i in 0..parts.len() - 1 {
                let dir = parts[..=i].join("/");
                let collapsed = self.collapsed.contains(&dir);
                if seen.insert(dir.clone()) {
                    rows.push(TreeRow {
                        depth: i,
                        name: parts[i].to_string(),
                        path: dir,
                        is_dir: true,
                        collapsed,
                    });
                }
                if collapsed {
                    continue 'files;
                }
            }
            rows.push(TreeRow {
                depth: parts.len() - 1,
                name: parts[parts.len() - 1].to_string(),
                path: path.clone(),
                is_dir: false,
                collapsed: false,
            });
        }
        self.rows = rows;
    }

    fn list_scroll_mut(&mut self) -> &mut usize {
        match self.mode {
            Mode::Changes => &mut self.scroll_changes,
            Mode::Files => &mut self.scroll_files,
        }
    }

    fn fix_scroll(&mut self) {
        let layout = match self.layout {
            Some(l) => l,
            None => return,
        };
        let inner = layout.box_h.saturating_sub(2).max(1);
        let index = self.current_index();
        let len = self.list_len();
        let follow = self.follow;
        {
            let scroll = self.list_scroll_mut();
            if follow {
                if index < *scroll {
                    *scroll = index;
                } else if index >= *scroll + inner {
                    *scroll = index + 1 - inner;
                }
            }
            *scroll = (*scroll).min(len.saturating_sub(inner));
        }
        self.view_scroll = self.view_scroll.min(self.view.len().saturating_sub(inner));
    }

    // ---- git actions (handed to the worker, UI keeps running) ------------

    fn commit(&mut self, push: bool) {
        if self.busy.is_some() {
            return;
        }
        let message = self.message.trim().to_string();
        if self.entries.is_empty() {
            self.set(Level::Info, "Nothing to commit");
            return;
        }
        if message.is_empty() {
            self.set(Level::Err, "Write a commit message first");
            return;
        }
        self.busy = Some(if push && self.has_remote {
            "Committing and pushing...".to_string()
        } else {
            "Committing...".to_string()
        });
        let _ = self.tx.send(Cmd::Commit { message, push });
    }

    fn push(&mut self) {
        if self.busy.is_some() {
            return;
        }
        if !self.has_remote {
            self.set(Level::Err, "No remote configured");
            return;
        }
        self.busy = Some("Pushing...".to_string());
        let _ = self.tx.send(Cmd::Push);
    }

    fn pull(&mut self) {
        if self.busy.is_some() {
            return;
        }
        if !self.has_remote {
            self.set(Level::Err, "No remote configured");
            return;
        }
        self.busy = Some("Pulling...".to_string());
        let _ = self.tx.send(Cmd::Pull);
    }

    // ---- input -----------------------------------------------------------

    fn page_size(&self) -> usize {
        self.layout
            .map(|l| l.box_h.saturating_sub(3).max(1))
            .unwrap_or(10)
    }

    /// Returns true when the user asked to quit.
    fn on_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => return true,
            KeyCode::Char('s') if ctrl => self.commit(false),
            KeyCode::Char('p') if ctrl => self.push(),
            KeyCode::Char('l') if ctrl => self.pull(),
            KeyCode::Char('u') if ctrl => self.message.clear(),
            KeyCode::Char(c) if !ctrl => self.message.push(c),
            KeyCode::Backspace => {
                self.message.pop();
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => {
                self.view_scroll = self.view_scroll.saturating_sub(self.page_size());
            }
            KeyCode::PageDown => {
                self.view_scroll += self.page_size();
            }
            KeyCode::Home => self.view_scroll = 0,
            KeyCode::End => self.view_scroll = usize::MAX / 2,
            KeyCode::Tab => {
                let next = if self.mode == Mode::Changes {
                    Mode::Files
                } else {
                    Mode::Changes
                };
                self.switch_mode(next);
            }
            KeyCode::Right => {
                if let Some((path, true)) = self.selected_is_dir() {
                    self.set_collapsed(&path, false);
                }
            }
            KeyCode::Left => {
                if let Some((path, false)) = self.selected_is_dir() {
                    self.set_collapsed(&path, true);
                }
            }
            KeyCode::Enter => {
                if self.message.trim().is_empty() {
                    if let Some((path, collapsed)) = self.selected_is_dir() {
                        self.set_collapsed(&path, !collapsed);
                    } else {
                        self.set(Level::Info, "Type a commit message, then press Enter");
                    }
                } else {
                    self.commit(true);
                }
            }
            KeyCode::Esc => {
                if self.message.is_empty() {
                    return true;
                }
                self.message.clear();
            }
            _ => {}
        }
        false
    }

    /// Wheel acceleration: slow flicks move a few lines, fast bursts move more.
    fn wheel_step(&mut self) -> usize {
        let now = Instant::now();
        let burst = self
            .last_wheel
            .map_or(false, |t| now.duration_since(t) < WHEEL_BURST);
        self.wheel_streak = if burst { self.wheel_streak + 1 } else { 0 };
        self.last_wheel = Some(now);
        WHEEL_BASE_STEP + (self.wheel_streak / 2).min(WHEEL_MAX_BOOST)
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let layout = match self.layout {
            Some(l) => l,
            None => return,
        };
        let (col, row) = (mouse.column as usize, mouse.row as usize);
        let in_boxes_rows = row >= 1 && row <= layout.box_h;
        let in_left = in_boxes_rows && col < layout.left_w;
        let in_right = in_boxes_rows && col >= layout.left_w && col < layout.width;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if in_left => {
                if row == 1 {
                    if col >= layout.tab_changes.0 && col < layout.tab_changes.1 {
                        self.switch_mode(Mode::Changes);
                    } else if col >= layout.tab_files.0 && col < layout.tab_files.1 {
                        self.switch_mode(Mode::Files);
                    }
                } else if row >= 2 && row < layout.box_h {
                    let scroll = match self.mode {
                        Mode::Changes => self.scroll_changes,
                        Mode::Files => self.scroll_files,
                    };
                    let index = scroll + row - 2;
                    if index < self.list_len() {
                        self.select_index(index);
                        if let Some((path, collapsed)) = self.selected_is_dir() {
                            self.set_collapsed(&path, !collapsed);
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp if in_left => {
                let step = self.wheel_step();
                self.follow = false;
                let scroll = self.list_scroll_mut();
                *scroll = scroll.saturating_sub(step);
            }
            MouseEventKind::ScrollDown if in_left => {
                let step = self.wheel_step();
                self.follow = false;
                *self.list_scroll_mut() += step;
            }
            MouseEventKind::ScrollUp if in_right => {
                let step = self.wheel_step();
                self.view_scroll = self.view_scroll.saturating_sub(step);
            }
            MouseEventKind::ScrollDown if in_right => {
                let step = self.wheel_step();
                self.view_scroll += step;
            }
            _ => {}
        }
    }
}

// ============================================================================
// RENDERING
// ============================================================================

fn paint(text: &str, fg: Color, bg: Option<Color>, bold: bool) -> String {
    let mut styled = text.with(fg);
    if let Some(bg) = bg {
        styled = styled.on(bg);
    }
    if bold {
        styled = styled.bold();
    }
    styled.to_string()
}

fn truncate(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max - 1).collect::<String>())
    }
}

/// Keeps the end of the text (useful for paths).
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if max == 0 {
        return String::new();
    }
    if count <= max {
        text.to_string()
    } else {
        let skip = count - (max - 1);
        format!("…{}", text.chars().skip(skip).collect::<String>())
    }
}

fn pad(text: &str, width: usize) -> String {
    let cut = truncate(text, width);
    let used = cut.chars().count();
    format!("{}{}", cut, " ".repeat(width.saturating_sub(used)))
}

fn spinner_frame(app: &App) -> &'static str {
    SPINNER[(app.started.elapsed().as_millis() / 100) as usize % SPINNER.len()]
}

/// "┌─ title ────┐" spanning exactly `width` columns. `visible` is the title's plain length.
fn top_border(width: usize, styled_title: &str, visible: usize, color: Color) -> String {
    let fill = width.saturating_sub(visible + 5);
    format!(
        "{}{}{}",
        paint("┌─ ", color, None, false),
        styled_title,
        paint(&format!(" {}┐", "─".repeat(fill)), color, None, false)
    )
}

fn bottom_border(width: usize, color: Color) -> String {
    paint(
        &format!("└{}┘", "─".repeat(width.saturating_sub(2))),
        color,
        None,
        false,
    )
}

fn side(color: Color) -> String {
    paint("│", color, None, false)
}

fn left_cells(app: &App, l: &Layout) -> Vec<String> {
    let inner_w = l.left_w - 2;
    let inner_h = l.box_h - 2;
    let mut out: Vec<String> = Vec::with_capacity(l.box_h);

    let changes_label = format!("Changes ({})", app.entries.len());
    let (s1, s2) = match app.mode {
        Mode::Changes => (
            paint(&changes_label, C_ACCENT, None, true),
            paint("Files", C_DIM, None, false),
        ),
        Mode::Files => (
            paint(&changes_label, C_DIM, None, false),
            paint("Files", C_ACCENT, None, true),
        ),
    };
    let title = format!("{}{}{}", s1, paint(" │ ", C_BORDER, None, false), s2);
    out.push(top_border(
        l.left_w,
        &title,
        changes_label.chars().count() + 3 + 5,
        C_BORDER,
    ));

    let codes: HashMap<&str, &str> = app
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.code.as_str()))
        .collect();
    let current = app.current_index();
    let scroll = match app.mode {
        Mode::Changes => app.scroll_changes,
        Mode::Files => app.scroll_files,
    };

    for i in 0..inner_h {
        let index = scroll + i;
        let selected = index == current;
        let bg = if selected { Some(C_SEL_BG) } else { None };

        let cell = match app.mode {
            Mode::Changes => {
                if app.entries.is_empty() {
                    let label = if app.loaded {
                        "Waiting for changes..."
                    } else {
                        "Loading..."
                    };
                    match i {
                        0 => format!(
                            " {} {}",
                            paint(spinner_frame(app), C_ACCENT, None, false),
                            paint(&pad(label, inner_w.saturating_sub(3)), Color::Reset, None, true)
                        ),
                        1 if app.loaded => paint(
                            &pad("   Edit a file and it appears here", inner_w),
                            C_DIM,
                            None,
                            false,
                        ),
                        _ => " ".repeat(inner_w),
                    }
                } else if let Some(e) = app.entries.get(index) {
                    let text_w = inner_w.saturating_sub(3);
                    format!(
                        "{}{}{}",
                        paint(" ", Color::Reset, bg, false),
                        paint(status_letter(&e.code), status_color(&e.code), bg, true),
                        paint(
                            &format!(" {}", pad(&tail(&e.path, text_w), text_w)),
                            Color::Reset,
                            bg,
                            false
                        )
                    )
                } else {
                    " ".repeat(inner_w)
                }
            }
            Mode::Files => {
                if let Some(r) = app.rows.get(index) {
                    let indent = " ".repeat(r.depth * 2);
                    if r.is_dir {
                        let icon = if r.collapsed { "▸ " } else { "▾ " };
                        let text = format!(" {}{}{}", indent, icon, r.name);
                        paint(&pad(&text, inner_w), C_DIR, bg, true)
                    } else {
                        let color = codes
                            .get(r.path.as_str())
                            .map(|c| status_color(c))
                            .unwrap_or(Color::Reset);
                        let text = format!(" {}  {}", indent, r.name);
                        paint(&pad(&text, inner_w), color, bg, false)
                    }
                } else {
                    " ".repeat(inner_w)
                }
            }
        };
        out.push(format!("{}{}{}", side(C_BORDER), cell, side(C_BORDER)));
    }

    out.push(bottom_border(l.left_w, C_BORDER));
    out
}

fn render_view_line(line: &ViewLine, width: usize) -> String {
    let number = line
        .new_no
        .or(line.old_no)
        .map(|n| format!("{:>5}", n))
        .unwrap_or_else(|| "     ".to_string());

    match line.kind {
        LineKind::Note => paint(&pad(&format!(" {}", line.text), width), C_DIM, None, false),
        LineKind::Add => paint(
            &pad(&format!("{} + {}", number, line.text), width),
            C_ADD_FG,
            Some(C_ADD_BG),
            false,
        ),
        LineKind::Del => paint(
            &pad(&format!("{} - {}", number, line.text), width),
            C_DEL_FG,
            Some(C_DEL_BG),
            false,
        ),
        LineKind::Ctx => format!(
            "{}{}",
            paint(&format!("{} ", number), C_DIM, None, false),
            paint(
                &pad(&format!("  {}", line.text), width.saturating_sub(6)),
                Color::Reset,
                None,
                false
            )
        ),
    }
}

fn right_cells(app: &App, l: &Layout) -> Vec<String> {
    let inner_w = l.right_w - 2;
    let inner_h = l.box_h - 2;
    let mut out: Vec<String> = Vec::with_capacity(l.box_h);

    let title_text = match &app.view_path {
        Some(p) => tail(p, inner_w.saturating_sub(8)),
        None => "Viewer".to_string(),
    };
    let visible = title_text.chars().count();
    out.push(top_border(
        l.right_w,
        &paint(&title_text, Color::Reset, None, true),
        visible,
        C_BORDER,
    ));

    for i in 0..inner_h {
        let cell = if app.view_path.is_none() {
            if i == 0 {
                paint(&pad("  Select a file to see its changes", inner_w), C_DIM, None, false)
            } else {
                " ".repeat(inner_w)
            }
        } else if let Some(line) = app.view.get(app.view_scroll + i) {
            render_view_line(line, inner_w)
        } else {
            " ".repeat(inner_w)
        };
        out.push(format!("{}{}{}", side(C_BORDER), cell, side(C_BORDER)));
    }

    out.push(bottom_border(l.right_w, C_BORDER));
    out
}

fn build_frame(app: &App, l: &Layout) -> (Vec<String>, u16, u16) {
    let mut lines: Vec<String> = vec![String::new(); l.height];

    // Header
    let mut sync = String::new();
    if app.ahead > 0 {
        sync.push_str(&paint(&format!("  ↑{}", app.ahead), C_MOD, None, true));
    }
    if app.behind > 0 {
        sync.push_str(&paint(&format!("  ↓{}", app.behind), C_DEL_FG, None, true));
    }
    lines[0] = format!(
        " {}  {}  {}{}",
        paint("syncgit", C_ACCENT, None, true),
        paint(&truncate(&app.repo_name, l.width / 3), Color::Reset, None, true),
        paint(&format!("⎇ {}", app.branch), Color::Magenta, None, false),
        sync
    );

    // Boxes
    let left = left_cells(app, l);
    let right = right_cells(app, l);
    for (i, (a, b)) in left.iter().zip(right.iter()).enumerate() {
        lines[1 + i] = format!("{}{}", a, b);
    }

    // Commit box
    let inner_w = l.width - 2;
    let title = paint("Commit message", C_ACCENT, None, true);
    lines[l.height - 4] = top_border(l.width, &title, 14, C_ACCENT);

    let text_w = inner_w.saturating_sub(3);
    let (shown, cursor_col) = if app.message.is_empty() {
        (
            paint(
                &pad("Type a message, then Enter to commit & push", text_w),
                C_DIM,
                None,
                false,
            ),
            4,
        )
    } else {
        let t = tail(&app.message, text_w.saturating_sub(1));
        let n = t.chars().count();
        (pad(&t, text_w), 4 + n)
    };
    lines[l.height - 3] = format!(
        "{} {} {}{}",
        side(C_ACCENT),
        paint("›", C_ACCENT, None, true),
        shown,
        side(C_ACCENT)
    );
    lines[l.height - 2] = bottom_border(l.width, C_ACCENT);

    // Hints / status / busy
    let hints = "↑↓ select · Tab Changes/Files · ←→ fold · PgUp/PgDn scroll · Enter commit+push · ^S commit · ^P push · ^L pull · Esc quit";
    lines[l.height - 1] = if let Some(label) = &app.busy {
        format!(
            " {} {}",
            paint(spinner_frame(app), C_ACCENT, None, true),
            paint(label, C_ACCENT, None, false)
        )
    } else {
        match &app.status {
            Some((level, text, at)) if at.elapsed() < MESSAGE_TTL => {
                let text = truncate(text, l.width.saturating_sub(4));
                match level {
                    Level::Info => format!(" {}", paint(&text, C_DIM, None, false)),
                    Level::Ok => format!(
                        " {} {}",
                        paint("✔", C_ADD_FG, None, true),
                        paint(&text, C_ADD_FG, None, false)
                    ),
                    Level::Err => format!(
                        " {} {}",
                        paint("✖", C_DEL_FG, None, true),
                        paint(&text, C_DEL_FG, None, false)
                    ),
                }
            }
            _ => format!(
                " {}",
                paint(&truncate(hints, l.width.saturating_sub(2)), C_DIM, None, false)
            ),
        }
    };

    (lines, cursor_col as u16, (l.height - 3) as u16)
}

/// Draws the frame, sending only the rows that changed since `prev`.
fn render(app: &mut App, prev: &mut Vec<String>) -> io::Result<()> {
    let (w, h) = terminal::size()?;
    let (width, height) = (w as usize, h as usize);

    app.layout = compute_layout(width, height, app.entries.len());
    app.fix_scroll();

    let mut buf: Vec<u8> = Vec::new();
    queue!(buf, Hide)?;

    match app.layout {
        None => {
            prev.clear();
            queue!(buf, Clear(ClearType::All), MoveTo(0, 0))?;
            write!(buf, "Terminal too small (need at least 60x12)")?;
        }
        Some(layout) => {
            let (lines, cursor_col, cursor_row) = build_frame(app, &layout);
            let full = prev.len() != lines.len();
            if full {
                queue!(buf, Clear(ClearType::All))?;
            }
            for (row, line) in lines.iter().enumerate() {
                if full || prev[row] != *line {
                    queue!(buf, MoveTo(0, row as u16))?;
                    write!(buf, "{}", line)?;
                    queue!(buf, Clear(ClearType::UntilNewLine))?;
                }
            }
            *prev = lines;
            queue!(buf, MoveTo(cursor_col, cursor_row), Show)?;
        }
    }

    let mut out = io::stdout();
    out.write_all(&buf)?;
    out.flush()
}

// ============================================================================
// ENTRY POINT
// ============================================================================

pub fn run(repo: &GitRepo) -> io::Result<()> {
    let _guard = TermGuard::enter()?;
    let mut app = App::new(repo);
    let mut prev: Vec<String> = Vec::new();
    let mut dirty = true;
    let mut last_frame: u128 = 0;

    'main: loop {
        if app.drain_updates() {
            dirty = true;
        }

        // Expire old status messages and advance spinners.
        if let Some((_, _, at)) = &app.status {
            if at.elapsed() >= MESSAGE_TTL {
                app.status = None;
                dirty = true;
            }
        }
        let frame = app.started.elapsed().as_millis() / 100;
        if frame != last_frame {
            last_frame = frame;
            if app.animating() {
                dirty = true;
            }
        }

        if dirty {
            render(&mut app, &mut prev)?;
            dirty = false;
        }

        if event::poll(INPUT_POLL)? {
            // Drain everything that is pending so mouse movement can't lag the UI.
            loop {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Press && app.on_key(key) {
                            break 'main;
                        }
                    }
                    Event::Mouse(mouse) => app.on_mouse(mouse),
                    Event::Resize(_, _) => prev.clear(),
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
            dirty = true;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_records() {
        assert_eq!(
            parse_entry(" M src/main.rs"),
            Some(Entry {
                code: "M".into(),
                path: "src/main.rs".into()
            })
        );
        assert_eq!(
            parse_entry("?? new file.txt"),
            Some(Entry {
                code: "??".into(),
                path: "new file.txt".into()
            })
        );
        assert_eq!(parse_entry(""), None);
    }

    #[test]
    fn truncates_text() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
        assert_eq!(tail("abcdef", 4), "…def");
        assert_eq!(pad("ab", 4), "ab  ");
    }

    #[test]
    fn parses_diff_with_line_numbers() {
        let diff = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";
        let lines = parse_diff(diff);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].kind, LineKind::Ctx);
        assert_eq!((lines[0].old_no, lines[0].new_no), (Some(1), Some(1)));
        assert_eq!(lines[1].kind, LineKind::Del);
        assert_eq!((lines[1].old_no, lines[1].new_no), (Some(2), None));
        assert_eq!(lines[2].kind, LineKind::Add);
        assert_eq!((lines[2].old_no, lines[2].new_no), (None, Some(2)));
        assert_eq!((lines[3].old_no, lines[3].new_no), (Some(3), Some(3)));
    }

    #[test]
    fn parses_ahead_behind_counts() {
        assert_eq!(parse_counts("2\t5\n"), Some((2, 5)));
        assert_eq!(parse_counts(""), None);
    }

    #[test]
    fn folders_sort_before_files() {
        let mut paths = vec!["src/ui.rs", "src/sub/x.rs", "README.md"];
        paths.sort_by_cached_key(|p| sort_key(p));
        assert_eq!(paths, vec!["src/sub/x.rs", "src/ui.rs", "README.md"]);
    }

    #[test]
    fn layout_requires_minimum_size() {
        assert!(compute_layout(40, 10, 0).is_none());
        let l = compute_layout(100, 30, 3).unwrap();
        assert_eq!(l.left_w + l.right_w, 100);
        assert_eq!(l.box_h, 25);
    }
}
