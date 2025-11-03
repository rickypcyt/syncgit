use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::env;
use std::path::PathBuf;
use std::net::TcpStream;

// Search upwards until finding .git
fn find_git_root(mut dir: PathBuf) -> Option<PathBuf> {
    loop {
        if dir.join(".git").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None; // Reached system root and no Git repo found
        }
    }
}

fn get_github_token() -> Option<String> {
    env::var("GITHUB_TOKEN").ok()
}

fn check_internet_connection() -> bool {
    // Try to connect to a DNS server (8.8.8.8) on port 53
    TcpStream::connect("8.8.8.8:53").is_ok()
}

fn center_text(text: &str) -> String {
    let width = term_size::dimensions().map(|(w, _)| w).unwrap_or(80);
    let padding = (width.saturating_sub(text.len())) / 2;
    format!("{}{}", " ".repeat(padding), text)
}

fn print_separator() {
    // Get terminal width, default to 80 if can't be determined
    let width = term_size::dimensions().map(|(w, _)| w).unwrap_or(80);
    println!("{}", "─".repeat(width));
}

fn run(cmd: &str, args: &[&str]) -> bool {
    let mut command = Command::new(cmd);
    
    // If it's a git command and we have a token, use it
    if cmd == "git" && get_github_token().is_some() {
        let token = get_github_token().unwrap();
        command.env("GITHUB_TOKEN", token);
    }
    
    let status = command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    match status {
        Ok(s) if s.success() => true,
        _ => {
            eprintln!("❌ Error executing: {} {:?}", cmd, args);
            false
        }
    }
}

fn main() {
    let current = env::current_dir().expect("❌ Could not get current directory");
    let git_root = find_git_root(current.clone());

    let repo_path = match git_root {
        Some(path) => path,
        None => {
            eprintln!("❌ You are not inside a Git repository");
            return;
        }
    };

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
