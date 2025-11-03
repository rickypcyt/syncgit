use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::env;
use std::path::{Path, PathBuf};
use std::net::TcpStream;
use std::fs;

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

fn get_github_token() -> Option<String> {
    env::var("GITHUB_TOKEN").ok()
}

fn check_internet_connection() -> bool {
    TcpStream::connect("8.8.8.8:53").is_ok()
}

fn center_text(text: &str) -> String {
    let width = term_size::dimensions().map(|(w, _)| w).unwrap_or(80);
    let padding = (width.saturating_sub(text.len())) / 2;
    format!("{}{}", " ".repeat(padding), text)
}

fn print_separator() {
    let width = term_size::dimensions().map(|(w, _)| w).unwrap_or(80);
    println!("{}", "─".repeat(width));
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

fn main() {
    let current = env::current_dir().expect("❌ Could not get current directory");
    let git_root = find_git_root(current.clone());

    let repo_path = match git_root {
        Some(path) => path,
        None => {
            // 🔄 NUEVO COMPORTAMIENTO: listar repos hijos
            println!("{}", center_text("📦 Searching for Git repositories in subfolders..."));
            print_separator();
            if !list_child_git_repos(&current) {
                eprintln!("❌ You are not inside a Git repository nor are there any Git repositories in child directories.");
            }
            return;
        }
    };

    // --- Tu flujo original aquí ---
    let repo_name = repo_path.file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("")).to_string_lossy();

    print_separator();
    println!("{}", center_text(&format!("📁 Repository root: {}", repo_name)));
    println!("{}", center_text(&format!("🗂️  Path: {}", repo_path.display())));
    print_separator();

    println!("{}", center_text("🔍 Repository status:"));
    if !run("git", &["status", "-sb"]) {
        return;
    }
    print_separator();

    println!("{}", center_text("⬇️  Pulling changes..."));
    if !run("git", &["pull"]) {
        return;
    }
    print_separator();

    println!("{}", center_text("📦 Checking local changes..."));
    let has_changes = !Command::new("git")
        .args(&["diff", "--quiet"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false) || !Command::new("git")
        .args(&["diff", "--cached", "--quiet"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false) || !Command::new("git")
        .args(&["ls-files", "--others", "--exclude-standard"])
        .output()
        .map(|output| output.stdout.is_empty())
        .unwrap_or(true);

    if has_changes {
        if run("git", &["add", "."]) {
            println!("{}", center_text("✅ Changes added"));
        } else {
            return;
        }
    } else {
        println!("{}", center_text("🟢 No changes to add"));
        return;
    }

    print_separator();
    print!("✏️  Enter your commit message: ");
    io::stdout().flush().unwrap();
    let mut mensaje = String::new();
    io::stdin().read_line(&mut mensaje).unwrap();
    let mensaje = mensaje.trim();

    if mensaje.is_empty() {
        eprintln!("{}", center_text("⚠️  Message cannot be empty"));
        return;
    }

    if !run("git", &["commit", "-m", mensaje]) {
        return;
    }
    print_separator();

    println!("{}", center_text("⬆️  Pushing changes..."));
    if !check_internet_connection() {
        println!("{}", center_text("⚠️  No internet connection. Changes have been saved locally but not pushed."));
        println!("{}", center_text("    Please run 'git push' manually when you have connection."));
        return;
    }

    if let Some(token) = get_github_token() {
        let remote_url = Command::new("git")
            .args(&["config", "--get", "remote.origin.url"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|url| url.trim().to_string());

        if let Some(url) = remote_url {
            let auth_url = url.replace("https://", &format!("https://{}@", token));
            if !run("git", &["remote", "set-url", "origin", &auth_url]) {
                return;
            }
        }
    }

    run("git", &["push"]);
}

