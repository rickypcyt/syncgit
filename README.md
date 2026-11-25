# 🛠️ syncgit

[![Crates.io](https://img.shields.io/crates/v/syncgit.svg)](https://crates.io/crates/syncgit)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**A live terminal UI for git.** Run `syncgit` inside any repository and it watches your files in real time: edit something and it shows up instantly, with the changes in green and red right inside the code. Type a message, press `Enter`, and your work is staged, committed and pushed.

Think of it as the Source Control panel of VS Code, but in your terminal and one keystroke away.

```
 syncgit  my-project  ⎇ main  ↑1
┌─ Changes (3) │ Files │ Log │ Br ────────┐┌─ src/main.rs ────────────────────────────┐
│ M src/main.rs                           ││   12   let total = 1;                    │
│ U src/watch.rs                          ││   13 - let name = "a";        (red)      │
│ D old.txt                               ││   13 + let name = "b";        (green)    │
└─────────────────────────────────────────┘└──────────────────────────────────────────┘
┌─ Commit message ─────────────────────────────────────────────────────────────────────┐
│ › Add branch switching                                                               │
└──────────────────────────────────────────────────────────────────────────────────────┘
 ↑↓ select · Tab next tab · ←→ fold · PgUp/PgDn scroll · Enter commit+push · Esc quit
```

## ✨ Features

- ⚡ **Real time**: the file list updates by itself; with nothing to commit it shows *All set, nothing to commit*.
- 🎨 **Code viewer with diffs**: whole files with added lines in green and removed lines in red, with line numbers. New files are all green.
- 🗂️ **Four tabs**, switch with `Tab` or the mouse:
  - **Changes**: modified, new, deleted and renamed files.
  - **Files**: a VS Code-style explorer (folders first, collapsible, respects `.gitignore`).
  - **Log**: the last 300 commits; select one to see its author, date, message and full colored diff.
  - **Branches**: local and remote branches, with the current one marked; switch with `Enter`.
- 🚀 **One-step commit & push**: type a message, press `Enter`.
- 🖱️ **Keyboard and mouse**: click files and tabs, scroll with the wheel (it accelerates when you scroll fast).
- 🧵 **Never freezes**: all git work runs on a background thread, so the UI stays responsive even during a push.
- 🔒 **Safe token handling**: your GitHub token is passed to git for a single command and never written to disk.
- 🌐 **Offline friendly**: commits are saved locally and the error is shown if the push fails.
- 🧭 **Classic mode** (`--classic`): the original step-by-step prompt flow, including creating a new GitHub repository.

## 🛠️ Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) 1.70 or later
- Git 2.23 or later (uses `git switch`)
- A terminal of at least **70×12** with truecolor support (Windows Terminal, iTerm2, Alacritty, kitty, VS Code terminal, ...)
- **Windows**: the MSVC linker, i.e. [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/) with the *Desktop development with C++* workload

### From crates.io

```bash
cargo install syncgit
```

### From source

```bash
git clone https://github.com/rickypcyt/syncgit.git
cd syncgit
cargo install --path .
```

### Update

```bash
cargo install syncgit --force
```

## 🚀 Usage

Run it from any folder inside a git repository:

```bash
syncgit            # live view (default)
syncgit --classic  # step-by-step prompts (alias: -c)
```

If the folder is not a repository, `syncgit` offers to initialize one (classic flow).

### Keys

| Key | Action |
| --- | --- |
| `↑` / `↓` | Select the previous / next item in the list |
| `Tab` | Next tab: Changes → Files → Log → Branches |
| `←` / `→` | Collapse / expand the selected folder (Files tab) |
| `PgUp` / `PgDn` | Scroll the viewer one page |
| `Home` / `End` | Jump to the top / bottom of the viewer |
| *type text* | Write the commit message |
| `Enter` | Commit and push everything (when there is a message) |
| `Enter` | On **Branches**, with an empty message: switch to the selected branch |
| `Enter` | On a folder, with an empty message: collapse / expand it |
| `Ctrl+S` | Commit only (no push) |
| `Ctrl+P` | Push |
| `Ctrl+L` | Pull (`--rebase --autostash`) |
| `Ctrl+U` | Clear the message |
| `Esc` | Clear the message, or quit if it is already empty |
| `Ctrl+C` | Quit |

### Mouse

- Click a tab title to switch tabs.
- Click a file, commit or branch to select it. Clicking a folder collapses or expands it.
- Use the wheel over the left box to scroll the list and over the right box to scroll the viewer.
- Hold `Shift` while dragging to select text in most terminals.

### Colors

| Mark | Meaning |
| --- | --- |
| `U` green | New (untracked) file |
| `A` green | Added to the index |
| `M` yellow | Modified |
| `D` red | Deleted |
| `R` / `C` cyan | Renamed / copied |
| `↑n` yellow | Commits ahead of the remote |
| `↓n` red | Commits behind the remote |

## 🔐 GitHub authentication

- **SSH remotes** (`git@github.com:...`) work out of the box with your SSH keys.
- **HTTPS remotes** use your configured credential helper, or a token from any of `GITHUB_TOKEN`, `GH_TOKEN` or `GIT_TOKEN`:

```bash
export GITHUB_TOKEN=your_token_here   # add it to your shell rc file to keep it
```

The token is passed to git through a process-local HTTP header. It is never written to `.git/config` or `.git/credentials`. Git is also run with `GIT_TERMINAL_PROMPT=0`, so it can never hang waiting for a password inside the UI.

## 🧪 Development

```bash
cargo fmt -- --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Project layout:

| File | Purpose |
| --- | --- |
| `src/main.rs` | Entry point, git helpers and the classic step-by-step flow |
| `src/watch.rs` | The live UI: worker thread, tabs, viewer, rendering, input |
| `src/ui.rs` | Colored output and prompts for classic mode |
| `src/error.rs` | Structured error type |
| `src/tests.rs` | Unit tests for the core helpers |

How the live view works: a background worker polls `git status` every second and refreshes the active tab's list every few seconds. The UI thread never runs git; it only draws, redrawing just the lines that changed, and reads keys and mouse events.

## 📎 Dependencies

- [`crossterm`](https://crates.io/crates/crossterm): raw terminal, colors, keyboard and mouse events.
- [`reqwest`](https://crates.io/crates/reqwest): GitHub API client (repository creation, connectivity check).
- [`serde_json`](https://crates.io/crates/serde_json): GitHub API payloads.

## 📝 Changelog

### 0.4.0
- New live view (now the default): real-time changes, boxed layout, code viewer with green/red diffs.
- New tabs: **Files** explorer, **Log** (commit history with diffs) and **Branches** (list and switch).
- Mouse support, with accelerated wheel scrolling.
- Commit, push and pull run in the background; the UI never blocks.
- Redraws only the lines that changed; far less flicker.
- Left-aligned, colored classic mode (`--classic`).
- Package metadata, README and docs refreshed.

### 0.3.0
- Safer push workflow with explicit confirmation.
- Token support via `GITHUB_TOKEN`, `GH_TOKEN` or `GIT_TOKEN` without writing it to disk.
- Code split into modules, structured errors and unit tests.

### 0.1.6
- Grouped subpath status for parent folders with many projects.
- Staging limited to the current subpath; consistent `git -C <root>` usage.

## 🤝 Contributing

Pull requests and feedback are welcome. Please open an issue first to discuss any major change.

## 📄 License

MIT © Rickypcyt

Made with ❤️ in Rust.
