use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::env;
use std::path::{Path, PathBuf};
use std::net::TcpStream;
use std::collections::BTreeMap;
use std::time::Duration;
use std::fs;

use crossterm::terminal;

// ============================================================================
// CONSTANTS
// ============================================================================

const MSG_NO_INTERNET_PUSH: &str = "⚠️  No internet connection. Changes have been saved locally but not pushed.";
const MSG_RUN_PUSH_MANUALLY: &str = "    Please run 'git push' manually when you have connection.";

const TOKEN_ENV_VARS: &[&str] = &["GITHUB_TOKEN", "GH_TOKEN", "GIT_TOKEN"];
const INTERNET_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

// ============================================================================
// GITHUB AUTH FUNCTIONS
// ============================================================================

fn get_github_token() -> Option<String> {
    // Check environment variables for token
    for var in TOKEN_ENV_VARS {
        if let Ok(token) = std::env::var(var) {
            if !token.trim().is_empty() {
                return Some(token.trim().to_string());
            }
        }
    }
    None
}

fn check_internet_connection() -> bool {
    // Try to connect to a reliable server (Google's DNS)
    TcpStream::connect_timeout(
        &"8.8.8.8:53".parse().unwrap(),
        INTERNET_CHECK_TIMEOUT
    ).is_ok()
}

// ============================================================================
// ERROR HANDLING
// ============================================================================

use std::fmt;
use std::error::Error;

#[derive(Debug)]
enum GitError {
    NoChanges,
    NoCommitMessage,
    CommandFailed(String),
    NoToken,
    NoInternet,
    #[allow(dead_code)]
    Other(String),
}

impl Error for GitError {}

type Result<T = ()> = std::result::Result<T, GitError>;

// ============================================================================
// GIT OPERATIONS
// ============================================================================

struct GitRepo {
    root: PathBuf,
    name: String,
}

impl GitRepo {
    fn find_from_path(path: &Path) -> Option<Self> {
        let mut current = path.to_path_buf();
        loop {
            if current.join(".git").exists() {
                let name = Self::extract_repo_name(&current);
                return Some(GitRepo { root: current, name });
            }

            if !current.pop() {
                return None;
            }
        }
    }

    fn extract_repo_name(path: &Path) -> String {
        // Try remote URL first
        if let Some(url) = Self::get_remote_url(path) {
            if let Some(name) = Self::parse_repo_name_from_url(&url) {
                return name;
            }
        }
        
        // Fallback to directory name
        path.file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("unknown"))
            .to_string_lossy()
            .to_string()
    }

    fn get_remote_url(path: &Path) -> Option<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("config")
            .arg("--get")
            .arg("remote.origin.url")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string());
            
        if let Some(ref url) = output {
            if url.is_empty() {
                return None;
            }
        }
        output
    }

    fn has_remote(&self) -> bool {
        Self::get_remote_url(&self.root).is_some()
    }

    fn parse_repo_name_from_url(url: &str) -> Option<String> {
        let url = url.trim_end_matches(".git");
        url.rfind('/')
            .and_then(|idx| {
                let name = &url[idx + 1..];
                if name.is_empty() { None } else { Some(name.to_string()) }
            })
    }

    fn get_branch(&self) -> String {
        self.run_command(&["symbolic-ref", "--short", "HEAD"])
            .map(|_| String::new())
            .unwrap_or_else(|e| {
                eprintln!("Error getting branch: {}", e);
                "unknown".to_string()
            })
    }

    fn has_upstream(&self) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .arg("rev-parse")
            .arg("--abbrev-ref")
            .arg("--symbolic-full-name")
            .arg("@{u}")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn get_ahead_behind_count(&self) -> (usize, usize) {
        if !self.has_upstream() {
            return (0, 0);
        }

        let branch = self.get_branch();
        let upstream = format!("{}@{{u}}", branch);

        Command::new("git")
            .arg("-C").arg(&self.root)
            .args(&["rev-list", "--left-right", "--count", &format!("{}...{}", branch, upstream)])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let parts: Vec<&str> = s.trim().split_whitespace().collect();
                if parts.len() == 2 {
                    let behind = parts[0].parse().ok()?;
                    let ahead = parts[1].parse().ok()?;
                    Some((ahead, behind))
                } else {
                    None
                }
            })
            .unwrap_or((0, 0))
    }

    /// Normalizes a pathspec to prevent command injection
    fn normalize_pathspec(path: &str) -> String {
        // Remove newline and carriage return characters
        let clean = path.replace('\\', "/")  // Normalizar separadores
                      .replace("\n", "")
                      .replace("\r", "");
        
        // Eliminar referencias a .git para evitar escapes de directorio
        clean.replace("/.git/", "/GIT_ESCAPED/")
    }

    fn has_changes(&self, pathspec: Option<&str>) -> bool {
        // First check if the repository is valid
        if !self.root.exists() {
            return false;
        }

        let mut args = vec!["status", "--porcelain=v1", "-z"];
        
        // Procesar el pathspec si existe
        let normalized = pathspec.map(|p| Self::normalize_pathspec(p));
        
        if let Some(ref norm_path) = normalized {
            if !norm_path.is_empty() {
                // Usar -z para manejar correctamente espacios en nombres de archivo
                args.push("--");
                args.push(norm_path);
            }
        }

        // Use Command directly for more control over execution
        match Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(&args)
            .output() 
        {
            Ok(output) => {
                if !output.status.success() {
                    eprintln!("Error al verificar cambios: {}", 
                        String::from_utf8_lossy(&output.stderr));
                    return false;
                }
                // Verificar si hay salida (cambios)
                !output.stdout.is_empty()
            },
            Err(e) => {
                eprintln!("Error al ejecutar git status: {}", e);
                false
            }
        }
    }

    fn run_command_with_output(&self, args: &[&str]) -> Result<String> {
        // Same as run_command but returns the command's output
        let output = self.create_command(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| GitError::CommandFailed(format!("Failed to execute git command: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(GitError::CommandFailed(format!(
                "git command failed with status {}: {}\nError: {}",
                output.status,
                args.join(" "),
                stderr
            )));
        }

        String::from_utf8(output.stdout)
            .map_err(|e| GitError::CommandFailed(format!("Failed to parse command output: {}", e)))
            .map(|s| s.trim().to_string())
    }

    fn run_command(&self, args: &[&str]) -> Result<()> {
        // Verify that the root directory exists
        if !self.root.exists() {
            return Err(GitError::CommandFailed(format!(
                "Repository root directory does not exist: {}",
                self.root.display()
            )));
        }

        // Verificar que es un directorio
        if !self.root.is_dir() {
            return Err(GitError::CommandFailed(format!(
                "Repository root is not a directory: {}",
                self.root.display()
            )));
        }

        // Verificar permisos de lectura
        if std::fs::metadata(&self.root)
            .map_err(|e| GitError::CommandFailed(format!(
                "Cannot access repository directory {}: {}",
                self.root.display(), e
            )))?
            .permissions().readonly()
        {
            return Err(GitError::CommandFailed(format!(
                "Insufficient permissions to read repository: {}",
                self.root.display()
            )));
        }

        // Configure the command with piped I/O
        let child = self.create_command(args)
            .stdin(Stdio::null())  // No input from stdin
            .stdout(Stdio::piped())  // Capture stdout
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| GitError::CommandFailed(format!(
                "Failed to spawn git command: {}", e
            )))?;
            
        // Wait for the command to complete and capture output
        let output = child.wait_with_output()
            .map_err(|e| GitError::CommandFailed(format!(
                "Failed to wait for git command: {}", e
            )))?;

        // Log stderr if there was an error or if there's any output
        if !output.stderr.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if !stderr.is_empty() {
                eprintln!("git stderr: {}", stderr);
            }
        }

        // Log stdout if there's any output (only for non-sensitive commands)
        let sensitive_commands = ["push", "pull", "fetch", "remote"];
        let is_sensitive = args.iter().any(|&arg| sensitive_commands.contains(&arg));
        
        if !output.stdout.is_empty() && !is_sensitive {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !stdout.is_empty() {
                println!("{}", stdout);
            }
        }

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(GitError::CommandFailed(format!(
                "git command failed with status {}: git {}\nError: {}",
                output.status, args.join(" "), stderr.trim()
            )))
        }
    }

    fn create_command<'a, I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&self.root);
        
        // Add each argument separately to prevent injection
        for arg in args {
            cmd.arg(arg);
        }
        
        if let Some(token) = get_github_token() {
            cmd.env("GITHUB_TOKEN", token);
        }
        
        cmd
    }

    fn configure_auth_remote(&self) -> Result<()> {
        let token = match get_github_token() {
            Some(t) => {
                println!("🔑 Found GitHub token");
                t
            }
            None => {
                println!("ℹ️  No GitHub token found");
                println!("   Tried: {}", TOKEN_ENV_VARS.join(", "));
                return Ok(());
            }
        };

        let remote_url = Self::get_remote_url(&self.root)
            .ok_or_else(|| GitError::CommandFailed("Failed to get remote URL".to_string()))?;

        if remote_url.starts_with("https://") {
            // Configure the credentials helper to store in memory (cache)
            self.run_command(&["config", "--local", "credential.helper", "cache"])?;
            
            // Configure cache timeout (default 15 minutes)
            self.run_command(&["config", "--local", "credential.helper", "cache --timeout=3600"])?;
            
            // Configurar la URL remota sin credenciales
            self.run_command(&["remote", "set-url", "origin", &remote_url])?;
            
            // Configurar el helper de credenciales para almacenamiento temporal
            self.run_command(&["config", "--local", "credential.helper", "store --file=.git/credentials"])?;
            
            // Guardar las credenciales temporalmente
            let mut cmd = self.create_command(&["credential", "approve"]);
            let mut child = cmd
                .stdin(Stdio::piped())
                .spawn()
                .map_err(|e| GitError::CommandFailed(format!("Failed to spawn git credential command: {}", e)))?;
            
            if let Some(stdin) = child.stdin.as_mut() {
                writeln!(stdin, "url={}", remote_url)
                    .map_err(|e| GitError::CommandFailed(format!("Failed to write to git credential stdin: {}", e)))?;
                writeln!(stdin, "username={}", token)
                    .map_err(|e| GitError::CommandFailed(format!("Failed to write to git credential stdin: {}", e)))?;
                writeln!(stdin, "password=x-oauth-basic")
                    .map_err(|e| GitError::CommandFailed(format!("Failed to write to git credential stdin: {}", e)))?;
            }
            
            let status = child.wait()
                .map_err(|e| GitError::CommandFailed(format!("Failed to wait for git credential command: {}", e)))?;
            
            if !status.success() {
                return Err(GitError::CommandFailed("Failed to store credentials".to_string()));
            }
            
            println!("✅ Configured secure credential helper");
        } else if remote_url.starts_with("git@") {
            println!("ℹ️  Using SSH authentication (no token needed)");
        } else {
            println!("ℹ️  Remote already configured or using non-HTTPS protocol");
        }

        Ok(())
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::NoChanges => write!(f, "No changes to commit"),
            GitError::NoCommitMessage => write!(f, "No commit message provided"),
            GitError::CommandFailed(msg) => write!(f, "Command failed: {}", msg),
            GitError::NoToken => write!(f, "No GitHub token found"),
            GitError::NoInternet => write!(f, "No internet connection"),
            GitError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

// ============================================================================
// UI HELPERS
// ============================================================================

struct UI;

impl UI {
    fn center_text(text: &str) -> String {
        let width = terminal::size()
            .map(|(w, _)| w as usize)
            .unwrap_or_else(|_| 80);
        let padding = (width.saturating_sub(text.len())) / 2;
        format!("{}{}", " ".repeat(padding), text)
    }

    fn print_separator() {
        let width = terminal::size()
            .map(|(w, _)| w as usize)
            .unwrap_or_else(|_| 80);
        println!("{}", "─".repeat(width));
    }

    fn prompt_yes_no(question: &str) -> bool {
        print!("❓ {} (y/n): ", question);
        if let Err(e) = io::stdout().flush() {
            eprintln!("Error flushing stdout: {}", e);
            return false;
        }
        
        let mut response = String::new();
        if let Err(e) = io::stdin().read_line(&mut response) {
            eprintln!("Error reading input: {}", e);
            return false;
        }
        
        matches!(response.trim().to_lowercase().as_str(), "y" | "yes")
    }

    fn prompt_input(prompt: &str) -> String {
        print!("✏️  {}: ", prompt);
        if let Err(e) = io::stdout().flush() {
            eprintln!("Error flushing stdout: {}", e);
            return String::new();
        }
        
        let mut input = String::new();
        if let Err(e) = io::stdin().read_line(&mut input) {
            eprintln!("Error reading input: {}", e);
            return String::new();
        }
        
        input.trim().to_string()
    }

    fn wait_for_enter() -> bool {
        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
            Ok(_) => true,
            Err(_) => false
        }
    }
}

// ============================================================================
// STATUS DISPLAY
// ============================================================================

fn print_grouped_status(repo: &GitRepo, pathspec: &str) {
    let output = match repo.create_command(&["status", "--porcelain=v1", "--", pathspec]).output() {
        Ok(o) => o,
        Err(_) => return,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for line in stdout.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() { continue; }

        let mut parts = trimmed.splitn(2, ' ');
        let _status = parts.next().unwrap_or_default();
        let rest = parts.next().unwrap_or_default().trim_start();

        // Handle renames: "old -> new"
        let path_part = if let Some(arrow_idx) = rest.find(" -> ") {
            &rest[arrow_idx + 4..]
        } else {
            rest
        };

        let key = path_part
            .find('/')
            .map(|idx| path_part[..idx].to_string())
            .unwrap_or_else(|| ".".to_string());

        groups.entry(key).or_default().push(trimmed.to_string());
    }

    if groups.is_empty() {
        println!("{}", UI::center_text("🟢 No changes in current subpath"));
        return;
    }

    for (group, items) in groups {
        let display_name = if group == "." { "(root)" } else { &group };
        println!("{}", UI::center_text(&format!("📁 {}", display_name)));
        for item in items {
            println!("{}", item);
        }
        UI::print_separator();
    }
}

// ============================================================================
// WORKFLOW FUNCTIONS
// ============================================================================

fn compute_pathspec(repo_root: &Path, current: &Path) -> String {
    current
        .strip_prefix(repo_root)
        .ok()
        .and_then(|p| {
            let s = p.to_string_lossy().to_string();
            if s.is_empty() { None } else { Some(s) }
        })
        .unwrap_or_else(|| ".".to_string())
}

fn stage_and_commit(repo: &GitRepo, pathspec: &str) -> Result<()> {
    UI::print_separator();
    println!("{}", UI::center_text("📄 Changes to be staged:"));
    print_grouped_status(repo, pathspec);

    if !repo.has_changes(Some(pathspec)) {
        println!("{}", UI::center_text("🟢 No changes to add in the current folder"));
        return Err(GitError::NoChanges);
    }

    // Ask for confirmation before staging
    println!("\n{}", UI::center_text("Press Enter to stage these changes, or Ctrl+C to cancel..."));
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        println!("\n{}", UI::center_text("❌ Operation cancelled"));
        return Err(GitError::CommandFailed("User cancelled the operation".into()));
    }

    // Stage changes
    // Use -- to prevent pathspec from being interpreted as an option
    println!("\n{}", UI::center_text("⏳ Staging changes..."));
    repo.run_command(&["add", "--", pathspec])?;
    println!("{}", UI::center_text("✅ Changes added"));

    // Verify staged changes exist
    let has_staged = Command::new("git")
        .arg("-C")
        .arg(&repo.root)
        .arg("diff")
        .arg("--cached")
        .arg("--quiet")
        .arg("--")
        .arg(pathspec)
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);

    if !has_staged {
        println!("{}", UI::center_text("ℹ️  There's nothing to commit"));
        println!("{}", UI::center_text("   All changes are already committed"));
        return Err(GitError::NoChanges);
    }

    // Show staged changes
    UI::print_separator();
    println!("{}", UI::center_text("📝 Staged changes to be committed:"));
    repo.run_command(&["diff", "--cached", "--stat"])?;
    
    // Ask for confirmation before committing
    println!("\n{}", UI::center_text("Press Enter to commit these changes, or any other key to cancel"));
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() || 
       !input.trim().is_empty() {
        println!("\n{}", UI::center_text("❌ Commit cancelled"));
        return Err(GitError::CommandFailed("User cancelled the commit".into()));
    }

    UI::print_separator();
    let message = UI::prompt_input("Enter commit message (or leave empty to cancel)");

    if message.trim().is_empty() {
        println!("\n{}", UI::center_text("❌ Commit cancelled - no message provided"));
        return Err(GitError::NoCommitMessage);
    }

    // Use -- to prevent the message from being interpreted as an option
    repo.run_command(&["commit", "-m", &message, "--"])?;
    UI::print_separator();
    
    Ok(())
}

fn check_git_conflicts(repo: &GitRepo) -> Result<()> {
    // Check for merge conflicts
    let has_conflicts = Command::new("git")
        .arg("-C").arg(&repo.root)
        .args(&["diff", "--name-only", "--diff-filter=U"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .map_err(|e| GitError::CommandFailed(format!("Failed to check for merge conflicts: {}", e)))?;

    if has_conflicts {
        return Err(GitError::CommandFailed(
"You have unresolved conflicts. Please resolve them before continuing.".into()
        ));
    }

    // Check if there's a merge in progress
    let merge_head_exists = repo.root.join(".git/MERGE_HEAD").exists();
    if merge_head_exists {
        return Err(GitError::CommandFailed(
            "Hay un merge en progreso. Por favor, completa o aborta el merge antes de continuar.".into()
        ));
    }

    // Verificar si hay stash pendiente
    let has_stash = Command::new("git")
        .arg("-C").arg(&repo.root)
        .args(&["stash", "list"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .map_err(|e| GitError::CommandFailed(format!("Failed to check for stashed changes: {}", e)))?;

    if has_stash {
        println!("{}", UI::center_text("⚠️  Warning: You have stashed changes"));
        if !UI::prompt_yes_no("Do you want to continue anyway?") {
            return Err(GitError::CommandFailed("Operation cancelled by user".into()));
        }
    }

    Ok(())
}

fn handle_pending_pushes(repo: &GitRepo) -> Result<()> {
    // First check for any conflicts or problematic states
    if let Err(e) = check_git_conflicts(repo) {
        println!("\n{}", UI::center_text(" Verification error:"));
        println!("{}\n", UI::center_text(&e.to_string()));
        return Err(e);
    }

    let (ahead, _) = repo.get_ahead_behind_count();
    
    if ahead == 0 {
        println!("{}", UI::center_text(" No pending commits to push"));
        UI::print_separator();
        return Ok(());
    }

    println!("{}", UI::center_text(" WARNING: You have commits that need to be pushed"));
    println!("{}", UI::center_text(&format!("   {} commits ahead of remote repository", ahead)));
    println!("{}", UI::center_text("   This could cause conflicts or duplicate commits."));
    UI::print_separator();

    if !UI::prompt_yes_no("Do you want to push the existing commits first?") {
        println!("{}", UI::center_text("⚠️  Continuing with the new commit without pushing changes..."));
        UI::print_separator();
        return Ok(());
    }

    println!("{}", UI::center_text("⬆️  Pushing existing commits..."));
    
    if get_github_token().is_none() {
        println!("{}", UI::center_text("❌ Cannot push: GitHub token not found"));
        println!("{}", UI::center_text("   Please configure your GitHub token"));
        return Err(GitError::NoToken);
    }
    
    if !check_internet_connection() {
        println!("{}", UI::center_text("⚠️  No internet connection. Cannot push existing commits."));
        println!("{}", UI::center_text("    Please resolve this before making new commits."));
        return Err(GitError::NoInternet);
    }

    repo.configure_auth_remote()?;
    // Ensure push doesn't receive any unwanted parameters
    repo.run_command(&["push", "--"])?;
    
    println!("{}", UI::center_text("✅ Existing commits pushed successfully!"));
    UI::print_separator();
    
    Ok(())
}

// ============================================================================
// REPOSITORY INITIALIZATION
// ============================================================================

fn initialize_git_repo(path: &Path) -> Result<GitRepo> {
    // Initialize git repository with 'main' as default branch
    let output = Command::new("git")
        .arg("init")
        .arg("-b")
        .arg("main")
        .current_dir(path)
        .output()
        .map_err(|e| GitError::Other(format!("Failed to run git init: {}", e)))?;

    if !output.status.success() {
        return Err(GitError::Other("Failed to initialize Git repository".to_string()));
    }

    // Create initial commit
    let repo = GitRepo {
        root: path.to_path_buf(),
        name: path.file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("new-repo"))
            .to_string_lossy()
            .to_string(),
    };
    
    // Ensure we're on main branch (in case git init created master)
    // Note: With git init -b main, the branch should already be main,
    // but we check and rename if it's master (for older git versions)
    if let Ok(current_branch) = repo.run_command_with_output(&["rev-parse", "--abbrev-ref", "HEAD"]) {
        let branch_name = current_branch.trim();
        if branch_name == "master" {
            // Rename master to main only if it exists
            repo.run_command(&["branch", "-m", "master", "main"])
                .map_err(|e| GitError::Other(format!("Failed to rename branch to main: {}", e)))?;
        }
    }
    // If we can't determine the branch, that's okay - git init -b main should have created main

    // Create .gitignore if it doesn't exist
    let gitignore_path = path.join(".gitignore");
    if !gitignore_path.exists() {
        let default_gitignore = "# Default .gitignore for new repositories\n\
# OS generated files\n.DS_Store\n.DS_Store?\n._*\n.Spotlight-V100\n.Trashes\nehthumbs.db\nThumbs.db\n\n# Build artifacts\ntarget/\n**/*.rs.bk\nCargo.lock\n\n# Editor directories and files\n.idea\n.vscode\n*.swp\n*.swo\n*~";
        
        fs::write(&gitignore_path, default_gitignore)
            .map_err(|e| GitError::Other(format!("Failed to create .gitignore: {}", e)))?;
    }
    
    // Add all files and create initial commit
    repo.run_command(&["add", "--all"])?;
    
    // Check if there are any changes to commit
    if repo.has_changes(None) {
        repo.run_command(&["commit", "-m", "Initial commit"])?;
        println!("\n✅ Created initial commit");
    } else {
        println!("\nℹ️  No files to commit in the initial repository");
    }

    Ok(repo)
}

fn create_github_repo(repo: &GitRepo) -> Result<()> {
    if !check_internet_connection() {
        return Err(GitError::NoInternet);
    }

    let token = get_github_token().ok_or(GitError::NoToken)?;
    let default_repo_name = repo.root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("new-repo")
        .to_string();
    
    // Ask for repository name with default
    let repo_name = loop {
        let input_name = UI::prompt_input(&format!("Enter GitHub repository name [{}]: ", default_repo_name));
        let repo_name = if input_name.trim().is_empty() {
            default_repo_name.clone()
        } else {
            input_name.trim().to_string()
        };
        
        // Validate repository name (GitHub requirements: alphanumeric, -, _, and . only)
        if repo_name.is_empty() {
            println!("{}", UI::center_text("❌ Repository name cannot be empty. Please try again."));
            continue;
        }
        if !repo_name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.') {
            println!("{}", UI::center_text("❌ Repository name can only contain alphanumeric characters, hyphens, underscores, and dots. Please try again."));
            continue;
        }
        break repo_name;
    };

    // Ask for description
    let description = UI::prompt_input("Enter repository description (optional): ");
    
    // Ask if should be private
    let is_private = UI::prompt_yes_no("Should this repository be private?");
    
    println!("\n{}", UI::center_text("🔄 Creating GitHub repository..."));
    
    // Create repository using GitHub API
    let client = reqwest::blocking::Client::new();
    let mut request_body = serde_json::json!({
        "name": repo_name,
        "private": is_private,
    });
    
    if !description.trim().is_empty() {
        request_body["description"] = serde_json::Value::String(description.trim().to_string());
    }
    
    let response = client
        .post("https://api.github.com/user/repos")
        .header("User-Agent", "syncgit")
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github.v3+json")
        .json(&request_body)
        .send()
        .map_err(|e| GitError::Other(format!("Failed to send request to GitHub API: {}", e)))?;
    
    if !response.status().is_success() {
        let status = response.status();
        let error_msg = response.text().unwrap_or_else(|_| "Unknown error".to_string());
        
        // Check if repository already exists (422 status with "already exists" message)
        if status == 422 && error_msg.contains("already exists") {
            println!("\n{}", UI::center_text(&format!("⚠️  Repository '{}' already exists on GitHub", repo_name)));
            if UI::prompt_yes_no("Do you want to use the existing repository and push to it?") {
                // Get GitHub username from API
                let username = client
                    .get("https://api.github.com/user")
                    .header("User-Agent", "syncgit")
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Accept", "application/vnd.github.v3+json")
                    .send()
                    .and_then(|r| r.json::<serde_json::Value>())
                    .ok()
                    .and_then(|json| json["login"].as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "unknown".to_string());
                
                // Get the existing repository URL
                let existing_repo_url = format!("https://github.com/{}/{}", username, repo_name);
                
                // Add remote origin if it doesn't exist
                if !repo.has_remote() {
                    repo.run_command(&["remote", "add", "origin", &format!("{}.git", existing_repo_url)])?;
                } else {
                    // Update existing remote
                    repo.run_command(&["remote", "set-url", "origin", &format!("{}.git", existing_repo_url)])?;
                }
                
                // Get current branch and push
                let branch = repo.run_command_with_output(&["rev-parse", "--abbrev-ref", "HEAD"])
                    .map(|b| b.trim().to_string())
                    .unwrap_or_else(|_| "main".to_string());
                
                println!("\n{}", UI::center_text("🚀 Pushing to existing GitHub repository..."));
                repo.configure_auth_remote()?;
                repo.run_command(&["push", "-u", "origin", &branch])?;
                println!("\n{}", UI::center_text(&format!("✅ Successfully pushed to GitHub repository: {}", existing_repo_url)));
                return Ok(());
            } else {
                return Err(GitError::Other("Repository creation cancelled by user".to_string()));
            }
        }
        
        // Provide helpful error messages for common issues
        let detailed_error = if status == 401 {
            format!("Authentication failed. Please check your GitHub token. Error: {}", error_msg)
        } else if status == 422 {
            format!("Invalid repository name or repository already exists. Error: {}", error_msg)
        } else if status == 403 {
            format!("Permission denied. Your token may not have 'repo' scope. Error: {}", error_msg)
        } else {
            format!("GitHub API error (status {}): {}", status, error_msg)
        };
        
        return Err(GitError::Other(detailed_error));
    }
    
    let response_json: serde_json::Value = response.json()
        .map_err(|e| GitError::Other(format!("Failed to parse GitHub response: {}", e)))?;
    
    let repo_url = response_json["html_url"]
        .as_str()
        .ok_or_else(|| GitError::Other("Failed to get repository URL from GitHub response".to_string()))?;
    
    // Get current branch name (defaults to main)
    let branch = repo.run_command_with_output(&["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|b| b.trim().to_string())
        .unwrap_or_else(|_| "main".to_string());
    
    // Get the clone URL (SSH or HTTPS) from the response
    let clone_url = response_json["clone_url"]
        .as_str()
        .ok_or_else(|| GitError::Other("Failed to get clone URL from GitHub response".to_string()))?;
    
    // Add remote origin
    if let Err(e) = repo.run_command(&["remote", "add", "origin", clone_url]) {
        if let Ok(output) = repo.run_command_with_output(&["remote", "get-url", "origin"]) {
            println!("ℹ️  Remote 'origin' already exists: {}", output.trim());
            if !UI::prompt_yes_no("Do you want to update the existing remote URL?") {
                println!("\n⚠️  Using existing remote. You may need to manually set up tracking.");
                return Ok(());
            }
            repo.run_command(&["remote", "set-url", "origin", clone_url])?;
        } else {
            return Err(e);
        }
    }
    
    // Ask for initial commit message if there are no commits yet
    let has_commits = repo.run_command_with_output(&["rev-list", "--count", "--all"])
        .map(|output| output.trim() != "0")  // If output is not "0", then there are commits
        .unwrap_or(false);
        
    if !has_commits {
        let commit_message = UI::prompt_input("Enter initial commit message (or press Enter for 'Initial commit'): ");
        let commit_message = if commit_message.trim().is_empty() {
            "Initial commit"
        } else {
            commit_message.trim()
        };
        
        // Stage all files
        repo.run_command(&["add", "."])?;
        
        // Create initial commit
        repo.run_command(&["commit", "-m", commit_message])?;
        println!("\n✅ Created initial commit with message: {}", commit_message);
    }
    
    println!("\n🚀 Pushing to GitHub repository...");
    
    // First, try to push with -u (which sets upstream)
    match repo.run_command(&["push", "-u", "origin", &branch]) {
        Ok(_) => {
            println!("\n✅ Successfully pushed to GitHub repository: {}", repo_url);
            Ok(())
        },
        Err(e) => {
            println!("\n⚠️  Failed to push to remote repository: {}", e);
            
            // Try to fetch first in case the remote has changes
            println!("\n🔄 Fetching from remote...");
            if let Err(e) = repo.run_command(&["fetch"]) {
                println!("⚠️  Failed to fetch from remote: {}", e);
            }
            
            // Try to set up tracking with a more robust approach
            println!("\n🔗 Setting up tracking...");
            
            // Create commands with proper references to branch
            let branch_ref = branch.as_str();
            let setup_commands = [
                ("branch", vec!["--set-upstream-to".to_string(), format!("origin/{}", branch_ref), branch_ref.to_string()]),
                ("push", vec!["-u".to_string(), "origin".to_string(), branch_ref.to_string()]),
            ];
            
            for (cmd, args) in setup_commands.iter() {
                let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                if let Err(e) = repo.run_command(&args_refs) {
                    println!("⚠️  Command failed: git {} {}", cmd, args.join(" "));
                    println!("   Error: {}", e);
                }
            }
            
            // Final attempt to push
            if UI::prompt_yes_no("Would you like to try pushing again?") {
                if let Err(e) = repo.run_command(&["push"]) {
                    println!("\n❌ Final push attempt failed: {}", e);
                    println!("\nYou may need to manually set up tracking with these commands:");
                    println!("  git branch --set-upstream-to=origin/{} {}", branch, branch);
                    println!("  git push -u origin {}", branch);
                    return Err(GitError::Other("Failed to push to remote repository".to_string()));
                } else {
                    println!("\n✅ Successfully pushed to GitHub repository!");
                    return Ok(());
                }
            }
            
            Err(GitError::Other("Push to remote repository was not completed".to_string()))
        }
    }
}

// ============================================================================
// UTILITY FUNCTIONS
// ============================================================================

#[allow(dead_code)]
fn print_token_setup_instructions() {
    println!("{}", UI::center_text("❌ No GitHub token found in the environment"));
    println!("{}", UI::center_text("   Please set it up before continuing:"));
    println!();
    println!("   1. Create a Personal Access Token in GitHub with 'repo' scope");
    println!("   2. Add it to your shell configuration (e.g., ~/.zshrc):");
    println!("      export GITHUB_TOKEN=your_token_here");
    println!("   3. Reload your shell: source ~/.zshrc");
    println!();
    println!("   Alternatively, you can use SSH for authentication instead.");
}

// ============================================================================
// MAIN
// ============================================================================

fn check_sync_status(repo: &GitRepo) -> Result<()> {
    let (ahead, behind) = repo.get_ahead_behind_count();
    
    if ahead > 0 {
        println!("\n{}", UI::center_text("⚠️  You have unpushed changes:"));
        println!("{} commits ahead of remote", ahead);
        
        if check_internet_connection() {
            println!("\n{}", UI::center_text("Press Enter to push changes, or Ctrl+C to cancel"));
            if UI::wait_for_enter() {
                repo.configure_auth_remote()?;
                repo.run_command(&["push", "--"])?;
                println!("{}", UI::center_text("✅ Changes pushed successfully!"));
            }
        } else {
            println!("{}", UI::center_text("ℹ️  No internet connection. Changes will remain local for now."));
        }
    }
    
    if behind > 0 {
        println!("\n{}: {} commits behind remote", 
            UI::center_text("⚠️  Your local branch is behind"), 
            behind
        );
        
        if check_internet_connection() {
            println!("\n{}", UI::center_text(&format!("You have {} commits to sync from remote", behind)));
            println!("{}", UI::center_text("Press Enter to view and sync these changes, or Ctrl+C to cancel"));
            
            if !UI::wait_for_enter() {
                println!("\n{}", UI::center_text("❌ Sync cancelled"));
                return Ok(());
            }
            
            // Show what will be synced
            if let Ok(output) = repo.run_command(&["log", "--oneline", "--graph", "--decorate", "--all", "-n", "5", "--no-merges", "--"]) {
                println!("\n{}", UI::center_text("Latest changes to sync:"));
                println!("{:?}", output);
            }
            
            println!("\n{}", UI::center_text("Press Enter to confirm sync, or Ctrl+C to cancel"));
            if !UI::wait_for_enter() {
                println!("\n{}", UI::center_text("❌ Sync cancelled"));
                return Ok(());
            }
            
            println!("\n{}", UI::center_text("🔄 Syncing changes..."));
            
            // First fetch the latest changes
            repo.run_command(&["fetch", "origin"])?;
            
            // Stash any local changes temporarily
            let has_stash = repo.run_command(&["stash", "push", "--include-untracked"]).is_ok();
            
            // Get current branch's upstream
            let upstream = match repo.run_command_with_output(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]) {
                Ok(upstream) => upstream,
                Err(_) => "origin/main".to_string()
            };
            
            // Reset to match the upstream branch
            repo.run_command(&["reset", "--hard", &upstream])?;
            
            // If we had stashed changes, apply them back
            if has_stash {
                repo.run_command(&["stash", "pop"])?;
            }
            
            println!("✅ Successfully synced with remote!");
        } else {
            println!("\n{}", UI::center_text("ℹ️  No internet connection. Working with local version for now."));
        }
    }
    
    if ahead == 0 && behind == 0 {
        println!("\n{}", UI::center_text("✅ Your repository is in sync with remote"));
    }
    
    Ok(())
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Get current directory
    let current_dir = env::current_dir()
        .map_err(|e| GitError::Other(format!("Failed to get current directory: {}", e)))?;

    // Try to find existing git repo or initialize a new one
    let repo = match GitRepo::find_from_path(&current_dir) {
        Some(repo) => repo,
        None => {
            println!("No Git repository found in current directory or its parents.");
            println!("Do you want to initialize a new Git repository here? (y/n)");
            
            let mut input = String::new();
            io::stdin().read_line(&mut input)
                .map_err(|e| GitError::Other(format!("Failed to read input: {}", e)))?;
            
            if input.trim().to_lowercase() == "y" {
                let new_repo = initialize_git_repo(&current_dir)?;
                
                // Ask if user wants to create GitHub repository
                UI::print_separator();
                if UI::prompt_yes_no("Do you want to create a GitHub repository and push to it?") {
                    if let Err(e) = create_github_repo(&new_repo) {
                        println!("\n{}: {}", UI::center_text("⚠️  Warning"), e);
                        println!("{}", UI::center_text("You can create the repository manually later."));
                        UI::print_separator();
                    } else {
                        println!("\n{}", UI::center_text("✅ Repository created and pushed to GitHub!"));
                        UI::print_separator();
                        return Ok(());
                    }
                }
                
                new_repo
            } else {
                println!("Exiting...");
                return Ok(());
            }
        }
    };

    // Check sync status at startup
    if let Err(e) = check_sync_status(&repo) {
        println!("\n{}: {}", UI::center_text("⚠️  Warning"), e);
        // Continue execution even if sync check fails
    }

    UI::print_separator();
    println!("{}", UI::center_text(&format!("📁 Repository root: {}", repo.name)));
    UI::print_separator();

    let pathspec = compute_pathspec(&repo.root, &current_dir);
    let subpath_display = if pathspec == "." {
        ". (repo root)".to_string()
    } else {
        pathspec.clone()
    };
    println!("{}", UI::center_text(&format!("🧭 Subpath: {}", subpath_display)));
    UI::print_separator();

    // Show status
    println!("{}", UI::center_text("🔍 Repository status:"));
    // Status is safe as it doesn't use user input
    repo.run_command(&["status", "--", "-sb"])?;
    UI::print_separator();

    // Check pending pushes
    println!("{}", UI::center_text("🔍 Checking for pending pushes..."));
    handle_pending_pushes(&repo)?;

    // Pull only if remote exists
    if repo.has_remote() {
        println!("{}", UI::center_text("⬇️  Pulling changes..."));
        // Pull is safe as it doesn't directly use user input
        if let Err(e) = repo.run_command(&["pull", "--"]) {
            // If pull fails due to no upstream, that's okay for new repos
            let error_msg = e.to_string();
            if !error_msg.contains("no upstream configured") && !error_msg.contains("no tracking information") {
                return Err(Box::new(e) as Box<dyn std::error::Error>);
            }
            // Otherwise, just continue
        }
        UI::print_separator();
    } else {
        println!("{}", UI::center_text("ℹ️  No remote configured. Skipping pull."));
        UI::print_separator();
    }

    // Check for changes
    println!("{}", UI::center_text("📦 Checking local changes..."));
    
    let all_changes = repo.has_changes(None);
    let current_changes = repo.has_changes(Some(&pathspec));

    if all_changes && !current_changes {
        println!("{}", UI::center_text("ℹ️  No changes detected in the current folder"));
        println!("{}", UI::center_text("   However, there are pending changes elsewhere in the repository."));
        println!("{}", UI::center_text("   Tip: run this tool from the repo root or navigate to the folder with changes."));
        return Ok(());
    }

    // Stage and commit
    stage_and_commit(&repo, &pathspec)?;

    // Only push if remote exists
    if repo.has_remote() {
        // Ask for confirmation before pushing
        println!("\n{}", UI::center_text("⚠️  You're about to push your changes to the remote repository."));
        println!("{}", UI::center_text("   Press Enter to confirm push, or Ctrl+C to cancel"));
        
        if !UI::wait_for_enter() {
            println!("\n{}", UI::center_text("❌ Push cancelled"));
            return Ok(());
        }
        
        println!("\n{}", UI::center_text("⬆️  Pushing changes..."));
        
        if !check_internet_connection() {
            println!("{}", UI::center_text(MSG_NO_INTERNET_PUSH));
            println!("{}", UI::center_text(MSG_RUN_PUSH_MANUALLY));
            return Ok(());
        }

        repo.configure_auth_remote()?;

        // Ensure push doesn't receive any unwanted parameters
        repo.run_command(&["push", "--"])?;
        println!("\n{}", UI::center_text("✅ Changes pushed successfully!"));
    } else {
        println!("\n{}", UI::center_text("ℹ️  No remote configured. Changes committed locally."));
        if UI::prompt_yes_no("Do you want to create a GitHub repository and push to it?") {
            create_github_repo(&repo)?;
        }
    }
    
    Ok(())
}