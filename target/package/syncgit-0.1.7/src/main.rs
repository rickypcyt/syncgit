use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::env;
use std::path::{Path, PathBuf};
use std::net::TcpStream;
use std::fs;

// Terminal sizing only
use crossterm::terminal;

// Reused messages
const MSG_NO_INTERNET_PUSH: &str = "⚠️  No internet connection. Changes have been saved locally but not pushed.";
const MSG_RUN_PUSH_MANUALLY: &str = "    Please run 'git push' manually when you have connection.";

fn find_git_root(mut dir: PathBuf) -> Option<PathBuf> {
    loop {
        if dir.join(".git").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Stage and commit changes limited to the provided pathspec.
/// Shows grouped status, stages changes, confirms staged diff exists, and prompts for commit message.
/// Returns true if commit was created; false to abort the flow.
fn stage_and_commit_current_pathspec(repo_path: &Path, pathspec: &str) -> bool {
    // Show grouped concise status for the current subpath
    print_separator();
    println!("{}", center_text("📄 Changes limited to current subpath:"));
    print_grouped_status(repo_path, pathspec);

    // Check if there are any changes in current pathspec
    let has_unstaged_in_current = !Command::new("git")
        .arg("-C").arg(repo_path)
        .args(&["diff", "--quiet", "--", pathspec])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let has_staged_in_current = !Command::new("git")
        .arg("-C").arg(repo_path)
        .args(&["diff", "--cached", "--quiet", "--", pathspec])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let has_untracked_in_current = !Command::new("git")
        .arg("-C").arg(repo_path)
        .args(&["ls-files", "--others", "--exclude-standard", "--", pathspec])
        .output()
        .map(|o| o.stdout.is_empty())
        .unwrap_or(true);

    let has_changes_in_current = has_unstaged_in_current || has_staged_in_current || !has_untracked_in_current;

    if has_changes_in_current {
        // Stage only within the current directory
        if run("git", &["-C", &repo_path.to_string_lossy(), "add", pathspec]) {
            println!("{}", center_text("✅ Changes added"));
        } else {
            return false;
        }
    } else {
        println!("{}", center_text("🟢 No changes to add in the current folder"));
        return false;
    }

    // Ensure there are staged changes to commit (only in the current folder)
    let has_staged_changes = !Command::new("git")
        .arg("-C").arg(repo_path)
        .args(&["diff", "--cached", "--quiet", "--", pathspec])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !has_staged_changes {
        println!("{}", center_text("ℹ️  There's nothing to commit"));
        println!("{}", center_text("   All changes are already committed"));
        return false;
    }

    // Pause for user to review the short status before committing
    print_separator();
    print!("↩️  Press Enter to commit changes...");
    io::stdout().flush().unwrap();
    let mut _enter_to_commit = String::new();
    io::stdin().read_line(&mut _enter_to_commit).unwrap();

    print_separator();
    print!("✏️  Enter your commit message: ");
    io::stdout().flush().unwrap();
    let mut message = String::new();
    io::stdin().read_line(&mut message).unwrap();
    let message = message.trim();

    if message.is_empty() {
        eprintln!("{}", center_text("⚠️  Message cannot be empty"));
        return false;
    }

    if !run("git", &["-C", &repo_path.to_string_lossy(), "commit", "-m", message]) {
        return false;
    }
    print_separator();
    true
}

fn get_github_token() -> Option<String> {
    env::var("GITHUB_TOKEN").ok()
}

fn check_internet_connection() -> bool {
    TcpStream::connect("8.8.8.8:53").is_ok()
}

fn center_text(text: &str) -> String {
    let (width, _) = terminal::size().unwrap_or((80, 24));
    let padding = ((width as usize).saturating_sub(text.len())) / 2;
    format!("{}{}", " ".repeat(padding), text)
}

// Helper: create a rectangle centered within area `r` using percentages of the size
// TUI helpers removed

fn print_separator() {
    let (width, _) = terminal::size().unwrap_or((80, 24));
    println!("{}", "─".repeat(width as usize));
}

fn run(cmd: &str, args: &[&str]) -> bool {
    let mut command = Command::new(cmd);
    if cmd == "git" {
        if let Some(token) = get_github_token() {
            command.env("GITHUB_TOKEN", token);
        }
    }
    let status = command.args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    matches!(status, Ok(s) if s.success())
}

fn get_ahead_count(path: &Path) -> i32 {
    // Verificar si hay un upstream configurado
    let upstream = Command::new("git")
        .arg("-C").arg(path)
        .args(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());

    if upstream.is_none() {
        return 0;
    }

    // Obtener el nombre de la rama actual
    let branch = Command::new("git")
        .arg("-C").arg(path)
        .args(&["symbolic-ref", "--short", "HEAD"])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or("(no branch)".to_string());

    let branch = branch.trim();
    let up = upstream.as_ref().unwrap().trim();

    // Contar commits ahead del remoto
    let count = Command::new("git")
        .arg("-C").arg(path)
        .args(&["rev-list", "--left-right", "--count", &format!("{}...{}", branch, up)])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or("0 0".to_string());

    let parts: Vec<&str> = count.trim().split_whitespace().collect();
    if parts.len() == 2 {
        parts[1].parse().unwrap_or(0)
    } else {
        0
    }
}

fn check_pending_push(path: &Path) -> bool {
    get_ahead_count(path) > 0
}

/// Compute the pathspec relative to the repo root for the current directory.
fn compute_pathspec(repo_path: &Path, current: &Path) -> String {
    let rel = current
        .strip_prefix(repo_path)
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    if rel.is_empty() { ".".to_string() } else { rel }
}

/// Print the current subpath relative to the repo root.
fn show_subpath_info(repo_path: &Path, current: &Path) {
    let pathspec = compute_pathspec(repo_path, current);
    if pathspec == "." {
        println!("{}", center_text("🧭 Subpath: . (repo root)"));
    } else {
        println!("{}", center_text(&format!("🧭 Subpath: {}", pathspec)));
    }
}

/// Configure remote URL with token (if present) to allow https `git push` with auth.
/// Returns true if OK or if there is no token; false if it fails.
fn configure_auth_remote(repo_path: &Path) -> bool {
    if let Some(token) = get_github_token() {
        let remote_url = Command::new("git")
            .arg("-C").arg(repo_path)
            .args(&["config", "--get", "remote.origin.url"]) 
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|url| url.trim().to_string());

        if let Some(url) = remote_url {
            let auth_url = url.replace("https://", &format!("https://{}@", token));
            return run("git", &["-C", &repo_path.to_string_lossy(), "remote", "set-url", "origin", &auth_url]);
        }
    }
    true
}

/// Show repository short status (equivalent to `git status -sb`).
fn show_repo_status(repo_path: &Path) -> bool {
    println!("{}", center_text("🔍 Repository status:"));
    let ok = run("git", &["-C", &repo_path.to_string_lossy(), "status", "-sb"]);
    ok
}

/// Run `git pull` and print a header.
fn run_git_pull(repo_path: &Path) -> bool {
    println!("{}", center_text("⬇️  Pulling changes..."));
    if !run("git", &["-C", &repo_path.to_string_lossy(), "pull"]) { return false; }
    true
}

/// Handle interactive flow when there are pending pushes.
/// Returns false if the process should abort (e.g., user cannot push or chooses to stop), true to continue.
fn handle_pending_pushes(repo_path: &Path) -> bool {
    // Show detailed info about ahead commits
    let ahead_count = get_ahead_count(repo_path);
    println!("{}", center_text("⚠️  WARNING: You have commits that need to be pushed!"));
    println!("{}", center_text(&format!("   {} commits ahead of remote repository", ahead_count)));
    println!("{}", center_text("   This could lead to duplicate commits if you proceed."));
    print_separator();

    print!("❓ Do you want to push existing commits first? (y/n): ");
    io::stdout().flush().unwrap();
    let mut response = String::new();
    io::stdin().read_line(&mut response).unwrap();
    let response = response.trim().to_lowercase();

    if response == "y" || response == "yes" {
        println!("{}", center_text("⬆️  Pushing existing commits..."));
        if !check_internet_connection() {
            println!("{}", center_text("⚠️  No internet connection. Cannot push existing commits."));
            println!("{}", center_text("    Please resolve this before making new commits."));
            return false;
        }

        if !configure_auth_remote(repo_path) { return false; }
        if !run("git", &["-C", &repo_path.to_string_lossy(), "push"]) {
            println!("{}", center_text("❌ Failed to push existing commits. Aborting."));
            return false;
        }
        println!("{}", center_text("✅ Existing commits pushed successfully!"));
        print_separator();
        true
    } else {
        println!("{}", center_text("⚠️  Proceeding with new commit despite pending pushes..."));
        print_separator();
        true
    }
}

fn get_repo_name(path: &Path) -> String {
    // Try to get repository name from remote URL first
    let remote_url = Command::new("git")
        .arg("-C").arg(path)
        .args(&["config", "--get", "remote.origin.url"])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|url| url.trim().to_string());

    if let Some(url) = remote_url {
        // Extract repo name from the URL
        // Examples:
        // https://github.com/user/repo.git -> repo
        // https://github.com/user/repo -> repo
        // git@github.com:user/repo.git -> repo
        if let Some(name) = extract_repo_name_from_url(&url) {
            return name;
        }
    }

    // Fallback: use directory name when URL is unavailable or unparsable
    path.file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("unknown"))
        .to_string_lossy()
        .to_string()
}

fn extract_repo_name_from_url(url: &str) -> Option<String> {
    // Remove trailing .git if present
    let url = url.trim_end_matches(".git");
    
    // Buscar el último segmento después del último /
    if let Some(last_slash) = url.rfind('/') {
        let name = &url[last_slash + 1..];
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    
    None
}

fn git_repo_status(path: &Path) -> Option<(String, String)> {
    let branch = Command::new("git")
        .arg("-C").arg(path)
        .args(&["symbolic-ref", "--short", "HEAD"])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or("(no branch)".to_string());

    let dirty = Command::new("git")
        .arg("-C").arg(path)
        .args(&["status", "--porcelain"])
        .output().ok()
        .map(|o| !o.stdout.is_empty()).unwrap_or(false);

    let upstream = Command::new("git")
        .arg("-C").arg(path)
        .args(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());

    let (ahead, behind) = if let Some(up) = upstream {
        let branch = branch.trim();
        let up = up.trim();
        let count = Command::new("git")
            .arg("-C").arg(path)
            .args(&["rev-list", "--left-right", "--count", &format!("{}...{}", branch, up)])
            .output().ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or("0 0".to_string());
        let parts: Vec<&str> = count.trim().split_whitespace().collect();
        if parts.len() == 2 {
            (parts[1].to_string(), parts[0].to_string())
        } else {
            ("0".to_string(), "0".to_string())
        }
    } else {
        ("0".to_string(), "0".to_string())
    };

    let mut status = String::new();
    if dirty { status += "📝"; }
    if ahead != "0" { status += "⬆️"; }
    if behind != "0" { status += "⬇️"; }
    if status.is_empty() { status = "✅".to_string(); }

    Some((branch.trim().to_string(), status))
}

fn list_child_git_repos(base: &Path) -> bool {
    let mut found = false;
    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join(".git").exists() {
                found = true;
                let dir_name = path.file_name().unwrap_or_default().to_string_lossy();
                if let Some((branch, status)) = git_repo_status(&path) {
                    println!("{:<30} [{:>10}] {}", dir_name, branch, status);
                }
            }
        }
    }
    found
}

fn print_grouped_status(repo_path: &Path, pathspec: &str) {
    use std::collections::BTreeMap;

    let output = match Command::new("git")
        .arg("-C").arg(repo_path)
        .args(&["status", "--porcelain=v1", "--", pathspec])
        .output() {
            Ok(o) => o,
            Err(_) => return,
        };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for line in stdout.lines() {
        // Format examples:
        //  M path/to/file
        // MM path/to/file
        // A  path
        // ?? path
        // R  old -> new (we will show the full line under the group of the new path)
        let trimmed = line.trim_start();
        if trimmed.is_empty() { continue; }

        // Extract path after the two-status columns (first 2 chars plus a space)
        // Porcelain v1 guarantees that path starts at index >=3; for safety, find first space
        let mut parts = trimmed.splitn(2, ' ');
        let _status = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim_start();

        // Handle rename syntax: "old -> new"
        let path_part = if let Some(arrow_idx) = rest.find(" -> ") {
            &rest[arrow_idx + 4..]
        } else {
            rest
        };

        // Determine top-level folder key
        let key = match path_part.find('/') {
            Some(idx) => path_part[..idx].to_string(),
            None => ".".to_string(),
        };

        groups.entry(key).or_default().push(trimmed.to_string());
    }

    if groups.is_empty() {
        println!("{}", center_text("🟢 No changes in current subpath"));
        return;
    }

    for (group, items) in groups {
        println!("{}", center_text(&format!("📁 {}", if group == "." { "(root)".to_string() } else { group })));
        for item in items {
            println!("{}", item);
        }
        print_separator();
    }
}

fn main() {
    let current = env::current_dir().expect("❌ Could not get current directory");
    // Warn early if GITHUB_TOKEN is not configured
    if get_github_token().is_none() {
        println!("{}", center_text("⚠️  No GITHUB_TOKEN found in the environment."));
        println!("{}", center_text("   Set it before running: export GITHUB_TOKEN=your_token_here"));
        println!("{}", center_text("   Tip: create a Personal Access Token in GitHub with repo permissions."));
        print_separator();
    }
    let git_root = find_git_root(current.clone());

    let repo_path = match git_root {
        Some(path) => path,
        None => {
            println!("{}", center_text("📦 Searching for Git repositories in subfolders..."));
            print_separator();
            if !list_child_git_repos(&current) {
                eprintln!("❌ You are not inside a Git repository nor are there any Git repositories in child directories.");
            }
            return;
        }
    };

    // --- Tu flujo original aquí ---
    let repo_name = get_repo_name(&repo_path);

    print_separator();
    println!("{}", center_text(&format!("📁 Repository root: {}", repo_name)));
    print_separator();

    // Show from where we're operating (before status) — only Subpath as requested
    show_subpath_info(&repo_path, &current);
    print_separator();

    if !show_repo_status(&repo_path) {
        return;
    }
    print_separator();

    // Check if there are commits pending to push
    println!("{}", center_text("🔍 Checking for pending pushes..."));
    let pending_push = check_pending_push(&repo_path);
    if pending_push {
        if !handle_pending_pushes(&repo_path) { return; }
    } else {
        println!("{}", center_text("✅ No pending pushes detected"));
        print_separator();
    }

    if !run_git_pull(&repo_path) {
        return;
    }
    print_separator();

    println!("{}", center_text("📦 Checking local changes..."));
    // Determine the pathspec for the current subdirectory relative to repo root
    let pathspec = compute_pathspec(&repo_path, &current);

    // (Current path and subpath already shown above)

    // All changes in the repo (anywhere)
    let all_changes_exist = Command::new("git")
        .arg("-C").arg(&repo_path)
        .args(&["status", "--porcelain=v1"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    // Changes limited to the current directory
    let changes_in_current_exist = Command::new("git")
        .arg("-C").arg(&repo_path)
        .args(&["status", "--porcelain=v1", "--", &pathspec])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    // If there are changes in the parent but none in the current folder, warn and exit
    if all_changes_exist && !changes_in_current_exist {
        println!("{}", center_text("ℹ️  No changes detected in the current folder"));
        println!("{}", center_text("   However, there are pending changes elsewhere in the repository."));
        println!("{}", center_text("   Tip: run this tool from the repo root or navigate to the folder with changes."));
        return;
    }

    // Stage and commit only within current directory
    if !stage_and_commit_current_pathspec(&repo_path, &pathspec) { return; }

    println!("{}", center_text("⬆️  Pushing changes..."));
    if !check_internet_connection() {
        println!("{}", center_text(MSG_NO_INTERNET_PUSH));
        println!("{}", center_text(MSG_RUN_PUSH_MANUALLY));
        return;
    }
    if !configure_auth_remote(&repo_path) { return; }
    run("git", &["-C", &repo_path.to_string_lossy(), "push"]);
}

