---
# 🛠️ syncgit — Git Sync CLI

A lightweight Rust-based CLI to streamline everyday Git flows: detect the repo root, show a clear status, stage only what you need, commit, and push — with a clean, user-friendly terminal UI.

## 📋 Features

- 🔍 Auto-detects the repository root (`.git`).
- 🧭 Shows a minimal header: `Repository root` and `Subpath` (relative path from the repo root).
- ✅ Global short status (`git status -sb`).
- 📄 Subpath-only view grouped by folder (pretty-printed `--porcelain` limited to the current subpath).
- ➕ Stages changes only within the current subpath.
- ⏸️ Pause to review: “Press Enter to commit changes…” before asking the commit message.
- ✏️ Commit message prompt, then push.
- ⚠️ Pending pushes detection to avoid duplicate histories.
- 🌐 Offline-friendly: commits locally, defers push with a clear message.
- 🔐 Optional GitHub token (`GITHUB_TOKEN`) to push to private repos via HTTPS.
- 🧭 Works from repo root or any subfolder. If not in a repo, lists child repos.

All path-sensitive `git` operations use `git -C <repo_root>` for robust behavior regardless of where you run `syncgit`.

## 🧱 Requirements

- [Rust](https://www.rust-lang.org/tools/install)
- Git installed and configured.
- (Optional) Set a GitHub token as an environment variable for private repositories:

```
export GITHUB_TOKEN=your_token_here
```

## 🚀 Installation

Install Rust if you don’t have it yet:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then install the CLI globally:

```sh
cargo install syncgit
```


## 🧪 Usage

Run the tool from anywhere inside a Git repository:

```sh
syncgit
```

### Typical flow

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
