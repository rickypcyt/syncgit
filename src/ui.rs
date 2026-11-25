use std::io::{self, Write};

use crossterm::style::Stylize;
use crossterm::terminal;

const INDENT: &str = "  ";
const MAX_RULE_WIDTH: usize = 64;

pub struct UI;

// Some helpers (success/warn/error) are provided for upcoming call sites.
#[allow(dead_code)]
impl UI {
    fn term_width() -> usize {
        terminal::size().map(|(w, _)| w as usize).unwrap_or(80)
    }

    /// Left-aligned line with a small indent (replaces the old centered layout).
    pub fn line(text: &str) -> String {
        format!("{}{}", INDENT, text)
    }

    /// Short, dimmed horizontal rule that never spans the whole terminal.
    pub fn print_separator() {
        let width = Self::term_width().saturating_sub(INDENT.len() * 2);
        let width = width.min(MAX_RULE_WIDTH);
        println!("{}{}", INDENT, "─".repeat(width).dark_grey());
    }

    /// Bold section title, preceded by a blank line.
    pub fn section(title: &str) {
        println!("\n{}{}", INDENT, title.bold().cyan());
    }

    pub fn success(text: &str) {
        println!("{}{}", INDENT, format!("✔ {}", text).green());
    }

    pub fn warn(text: &str) {
        println!("{}{}", INDENT, format!("⚠ {}", text).yellow());
    }

    pub fn error(text: &str) {
        println!("{}{}", INDENT, format!("✖ {}", text).red());
    }

    pub fn info(text: &str) {
        println!("{}{}", INDENT, format!("ℹ {}", text).dark_grey());
    }

    /// Compact header: repo name, branch, subpath and ahead/behind counters.
    pub fn header(repo: &str, branch: &str, subpath: &str, ahead: usize, behind: usize) {
        println!();
        println!(
            "{}{} {}  {} {}",
            INDENT,
            "📁",
            repo.bold(),
            "⎇".dark_grey(),
            branch.magenta().bold()
        );
        let mut sync = String::new();
        if ahead > 0 {
            sync.push_str(&format!("  ↑{}", ahead).yellow().to_string());
        }
        if behind > 0 {
            sync.push_str(&format!("  ↓{}", behind).red().to_string());
        }
        if ahead == 0 && behind == 0 {
            sync.push_str(&"  in sync".green().to_string());
        }
        println!(
            "{}{}{}{}",
            INDENT,
            "   path ".dark_grey(),
            subpath,
            sync
        );
        Self::print_separator();
    }

    /// Prints one `git status --porcelain` entry, colored by its status code.
    pub fn status_entry(entry: &str) {
        let (code, path) = match entry.split_once(' ') {
            Some((c, p)) => (c, p.trim_start()),
            None => (entry, ""),
        };
        let badge = format!("{:<2}", code);
        let styled = if code.contains('?') || code.contains('A') {
            badge.green().bold()
        } else if code.contains('D') {
            badge.red().bold()
        } else if code.contains('R') || code.contains('C') {
            badge.cyan().bold()
        } else {
            badge.yellow().bold()
        };
        println!("{}{}{} {}", INDENT, INDENT, styled, path);
    }

    pub fn prompt_yes_no(question: &str) -> bool {
        print!("{}{} {} ", INDENT, "?".cyan().bold(), format!("{} [y/N]", question));
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

    pub fn prompt_input(prompt: &str) -> String {
        print!("{}{} {}: ", INDENT, "›".cyan().bold(), prompt);
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

    pub fn wait_for_enter() -> bool {
        let mut input = String::new();
        io::stdin().read_line(&mut input).is_ok()
    }
}
