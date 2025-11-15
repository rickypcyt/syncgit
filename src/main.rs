use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::env;
use std::path::{Path, PathBuf};
use std::net::TcpStream;
use std::collections::BTreeMap;
use std::time::Duration;

use crossterm::terminal;

// ============================================================================
// CONSTANTS
// ============================================================================

const MSG_NO_INTERNET_PUSH: &str = "⚠️  No internet connection. Changes have been saved locally but not pushed.";
const MSG_RUN_PUSH_MANUALLY: &str = "    Please run 'git push' manually when you have connection.";

const TOKEN_ENV_VARS: &[&str] = &["GITHUB_TOKEN", "GH_TOKEN", "GIT_TOKEN"];
const INTERNET_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

// ============================================================================
// ERROR HANDLING
// ============================================================================

use std::fmt;

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

type Result<T> = std::result::Result<T, GitError>;

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

    /// Normaliza un pathspec para evitar inyección de comandos
    fn normalize_pathspec(path: &str) -> String {
        // Eliminar caracteres de nueva línea y retorno de carro
        let clean = path.replace('\\', "/")  // Normalizar separadores
                      .replace("\n", "")
                      .replace("\r", "");
        
        // Eliminar referencias a .git para evitar escapes de directorio
        clean.replace("/.git/", "/GIT_ESCAPED/")
    }

    fn has_changes(&self, pathspec: Option<&str>) -> bool {
        // Verificar primero si el repositorio es válido
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

        // Usar Command directamente para tener más control sobre la ejecución
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

    fn run_command(&self, args: &[&str]) -> Result<()> {
        // Verificar que el directorio raíz existe
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
            // Configurar el helper de credenciales para almacenar en memoria (cache)
            self.run_command(&["config", "--local", "credential.helper", "cache"])?;
            
            // Configurar el tiempo de caché (por defecto 15 minutos)
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
            _ => write!(f, "An unknown error occurred"),
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
    // Usar -- para prevenir que pathspec sea interpretado como opción
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

    // Usar -- para prevenir que el mensaje sea interpretado como opción
    repo.run_command(&["commit", "-m", &message, "--"])?;
    UI::print_separator();
    
    Ok(())
}

fn check_git_conflicts(repo: &GitRepo) -> Result<()> {
    // Verificar conflictos de merge
    let has_conflicts = Command::new("git")
        .arg("-C").arg(&repo.root)
        .args(&["diff", "--name-only", "--diff-filter=U"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .map_err(|e| GitError::CommandFailed(format!("Failed to check for merge conflicts: {}", e)))?;

    if has_conflicts {
        return Err(GitError::CommandFailed(
            "Tienes conflictos sin resolver. Por favor, resuélvelos antes de continuar.".into()
        ));
    }

    // Verificar si hay un merge en progreso
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
        println!("{}", UI::center_text("⚠️  Advertencia: Tienes cambios guardados en stash"));
        if !UI::prompt_yes_no("¿Deseas continuar de todos modos?") {
            return Err(GitError::CommandFailed("Operación cancelada por el usuario".into()));
        }
    }

    Ok(())
}

fn handle_pending_pushes(repo: &GitRepo) -> Result<()> {
    // First check for any conflicts or problematic states
    if let Err(e) = check_git_conflicts(repo) {
        println!("\n{}", UI::center_text("❌ Verification error:"));
        println!("{}\n", UI::center_text(&e.to_string()));
        return Err(e);
    }

    let (ahead, _) = repo.get_ahead_behind_count();
    
    if ahead == 0 {
        println!("{}", UI::center_text("✅ No pending commits to push"));
        UI::print_separator();
        return Ok(());
    }

    println!("{}", UI::center_text("⚠️  WARNING: You have commits that need to be pushed"));
    println!("{}", UI::center_text(&format!("   {} commits ahead of remote repository", ahead)));
    println!("{}", UI::center_text("   This could cause conflicts or duplicate commits."));
    UI::print_separator();

    if !UI::prompt_yes_no("Do you want to push the existing commits first?") {
        println!("{}", UI::center_text("⚠️  Continuing with the new commit without pushing changes..."));
        UI::print_separator();
        return Ok(());
    }

    println!("{}", UI::center_text("⬆️  Subiendo commits existentes..."));
    
    if get_github_token().is_none() {
        println!("{}", UI::center_text("❌ No se puede subir: No se encontró el token de GitHub"));
        println!("{}", UI::center_text("   Por favor configura tu token de GitHub"));
        return Err(GitError::NoToken);
    }
    
    if !check_internet_connection() {
        println!("{}", UI::center_text("⚠️  No internet connection. Cannot push existing commits."));
        println!("{}", UI::center_text("    Please resolve this before making new commits."));
        return Err(GitError::NoInternet);
    }

    repo.configure_auth_remote()?;
    // Asegurarse de que push no reciba parámetros no deseados
    repo.run_command(&["push", "--"])?;
    
    println!("{}", UI::center_text("✅ Existing commits pushed successfully!"));
    UI::print_separator();
    
    Ok(())
}

// ============================================================================
// UTILITY FUNCTIONS
// ============================================================================

fn get_github_token() -> Option<String> {
    TOKEN_ENV_VARS
        .iter()
        .find_map(|var| env::var(var).ok())
        .filter(|token| !token.trim().is_empty())
}

fn check_internet_connection() -> bool {
    match "8.8.8.8:53".parse() {
        Ok(addr) => {
            match TcpStream::connect_timeout(&addr, INTERNET_CHECK_TIMEOUT) {
                Ok(_) => true,
                Err(e) => {
                    eprintln!("Error checking internet connection: {}", e);
                    false
                }
            }
        },
        Err(e) => {
            eprintln!("Error parsing IP address: {}", e);
            false
        }
    }
}

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
            
            // Get current branch name
            let branch = repo.get_branch();
            
            // Pull with rebase and autostash to handle local changes
            repo.run_command(&["pull", "--rebase", "--autostash", "origin", &branch])?;
            
            // Push any local changes that were rebased on top
            if ahead > 0 {
                println!("Pushing local changes after sync...");
                repo.run_command(&["push", "origin", &branch])?;
            }
            
            println!("✅ Successfully synced with remote!");
        }
    } else {
        println!("\n{}", UI::center_text("ℹ️  No internet connection. Working with local version for now."));
    }
    
    if ahead == 0 && behind == 0 {
        println!("\n{}", UI::center_text("✅ Your repository is in sync with remote"));
    }
    
    Ok(())
}

fn main() {
    // Check token early
    if get_github_token().is_none() {
        print_token_setup_instructions();
        return;
    }

    // Get current directory and find git repo
    let current_dir = match env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("❌ Could not get current directory: {}", e);
            return;
        }
    };

    let repo = match GitRepo::find_from_path(&current_dir) {
        Some(repo) => repo,
        None => {
            println!("{}", UI::center_text("❌ No Git repository found"));
            return;
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
    // Status es seguro ya que no usa entrada de usuario
    if repo.run_command(&["status", "--", "-sb"]).is_err() {
        return;
    }
    UI::print_separator();

    // Check pending pushes
    println!("{}", UI::center_text("🔍 Checking for pending pushes..."));
    if handle_pending_pushes(&repo).is_err() {
        return;
    }

    // Pull
    println!("{}", UI::center_text("⬇️  Pulling changes..."));
    // Pull es seguro ya que no usa entrada de usuario directamente
    if repo.run_command(&["pull", "--"]).is_err() {
        return;
    }
    UI::print_separator();

    // Check for changes
    println!("{}", UI::center_text("📦 Checking local changes..."));
    
    let all_changes = repo.has_changes(None);
    let current_changes = repo.has_changes(Some(&pathspec));

    if all_changes && !current_changes {
        println!("{}", UI::center_text("ℹ️  No changes detected in the current folder"));
        println!("{}", UI::center_text("   However, there are pending changes elsewhere in the repository."));
        println!("{}", UI::center_text("   Tip: run this tool from the repo root or navigate to the folder with changes."));
        return;
    }

    // Stage and commit
    if stage_and_commit(&repo, &pathspec).is_err() {
        return;
    }

    // Push
    println!("{}", UI::center_text("⬆️  Pushing changes..."));
    
    if !check_internet_connection() {
        println!("{}", UI::center_text(MSG_NO_INTERNET_PUSH));
        println!("{}", UI::center_text(MSG_RUN_PUSH_MANUALLY));
        return;
    }

    if repo.configure_auth_remote().is_err() {
        return;
    }

    // Asegurarse de que push no reciba parámetros no deseados
    let _ = repo.run_command(&["push", "--"]);
}