---
# 🛠️ syncgit — Git Sync CLI (v0.2.0)

A lightweight Rust-based CLI to streamline everyday Git workflows with enhanced safety and user experience. Automatically detects repository context, provides clear status, and guides you through the commit and sync process with intuitive prompts.

## 🚀 What's New in v0.2.0

- 🛡️ **Safer Push Workflow**: Added explicit confirmation before pushing changes
- 🔄 **Improved Sync**: Better handling of remote changes with clear prompts
- 🌍 **Fully Internationalized**: All user-facing messages now in English
- 🎯 **More Precise**: Better detection of repository state and changes
- 🛠️ **Bug Fixes**: Various stability improvements and edge case handling

## 📋 Features

- 🔍 Auto-detects the repository root (`.git`)
- 🧭 Context-aware: Shows repository root and current subpath
- 📊 Clear status overview with color-coded output
- 📂 Subpath-aware operations for precise changes
- ⏯️ Interactive workflow with clear prompts at each step
- 🔄 Smart sync that handles both push and pull scenarios
- 🔒 Secure credential handling with GitHub token support
- 🌐 Offline-friendly with clear status indicators
- 🧭 Works from any subdirectory within a repository

All path-sensitive `git` operations use `git -C <repo_root>` for robust behavior regardless of where you run `syncgit`.

## 🛠️ Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (latest stable version recommended)
- Git 2.0 or later

### Install from crates.io

```bash
cargo install syncgit
```

### Build from source

```bash
git clone https://github.com/rickypcyt/syncgit-rustcli.git
cd syncgit
cargo install --path .
```

### GitHub Token (for private repositories)

To work with private repositories, set your GitHub token:

```bash
export GITHUB_TOKEN=your_github_token_here
# Add to your shell's rc file to make it persistent
```

## 🚀 Basic Usage

Run the tool from any directory within a Git repository:

```bash
syncgit
```

The tool will guide you through:
1. Reviewing changes
2. Staging files
3. Committing with a message
4. Syncing with remote (if needed)

1) Info header: `Repository root` and `Subpath`.
2) Global short status.
3) Check pending pushes; optionally push first.
4) Pull changes.
5) Local changes check; show subpath-only grouped status.
6) Stage only current subpath (idempotent if already staged).
7) Pause: press Enter → ask for commit message.
8) Commit and push.

### Subpath grouping (visual aid)

When you’re in a parent folder with multiple projects, the subpath status is grouped by top-level folder. This keeps large changes readable and helps you focus on a particular folder’s changes when committing from the parent.

## 🌐 Offline Mode

If no internet connection is detected, changes are committed locally but not pushed. A message will inform you to push manually once online.

## 🔐 GitHub Token Authentication

To push to private GitHub repositories via HTTPS, the tool will use the `GITHUB_TOKEN` environment variable (if available) to authenticate securely by rewriting the remote URL temporarily.

## 📦 Update to latest version

- Update this CLI:

```sh
cargo install syncgit --force
```

- Or manage binaries with cargo-update:

```sh
cargo install cargo-update
cargo install-update -a
```

## 📎 Dependencies

- [`term_size`](https://crates.io/crates/term_size): For responsive terminal layout.
- Standard Rust `std::process`, `std::io`, `std::env`, and `std::net`.

## 📝 Changelog

- 0.1.6
  - Grouped subpath status printing for cleaner views in parent folders with many projects.
  - Staging limited to current subpath; consistent `git -C <root>` usage.
  - “Press Enter to commit changes…” pause before commit message.
  - Header simplified: `Repository root` + `Subpath`.
  - Various UX improvements and clearer outputs.

## 🤝 Contributions

Pull requests and feedback are welcome! Please open an issue first to discuss any major changes.

Made with ❤️ in Rust.

---
