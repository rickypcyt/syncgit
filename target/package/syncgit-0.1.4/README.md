---
# 🛠️ Git Sync CLI

A lightweight Rust-based CLI tool to automate common Git tasks such as detecting the repository root, checking status, pulling, committing, and pushing changes — all in a clean, user-friendly terminal interface.

## 📋 Features

- 🔍 Automatically finds the root of a Git repository.
- 🗂️ Displays the repository name and path.
- ✅ Checks repository status (`git status -sb`).
- ⬇️ Pulls the latest changes from the remote.
- 📦 Detects uncommitted changes and stages them.
- ✏️ Prompts for a commit message.
- ⬆️ Pushes commits to the remote (with optional GitHub token support).
- 🌐 Checks for internet connectivity before pushing.

## 🧱 Requirements

- [Rust](https://www.rust-lang.org/tools/install)
- Git installed and configured.
- (Optional) Set a GitHub token as an environment variable for private repositories:

```
export GITHUB_TOKEN=your_token_here
```

##Global Installation

To make the tool globally available:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo install syncgit 
```

Usage

Simply run the program in any folder containing a Git repository:

```syncgit```


## 🧪 Usage

Run the tool from anywhere inside a Git repository:

```sh
sync-git
```

### Sample Flow:

1. Finds the `.git` root directory.
2. Displays repository path and name.
3. Shows Git status.
4. Pulls changes from remote.
5. Detects and stages modified or untracked files.
6. Prompts for a commit message.
7. Commits and pushes the changes.

## 🌐 Offline Mode

If no internet connection is detected, changes are committed locally but not pushed. A message will inform you to push manually once online.

## 🔐 GitHub Token Authentication

To push to private GitHub repositories via HTTPS, the tool will use the `GITHUB_TOKEN` environment variable (if available) to authenticate securely by rewriting the remote URL temporarily.

## 📎 Dependencies

- [`term_size`](https://crates.io/crates/term_size): For responsive terminal layout.
- Standard Rust `std::process`, `std::io`, `std::env`, and `std::net`.

## 🤝 Contributions

Pull requests and feedback are welcome! Please open an issue first to discuss any major changes.

Made with ❤️ in Rust.

---
