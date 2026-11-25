use std::io::{self, Write};

use crossterm::terminal;

pub struct UI;

impl UI {
    pub fn center_text(text: &str) -> String {
        let width = terminal::size()
            .map(|(w, _)| w as usize)
            .unwrap_or_else(|_| 80);
        let padding = (width.saturating_sub(text.len())) / 2;
        format!("{}{}", " ".repeat(padding), text)
    }

    pub fn print_separator() {
        let width = terminal::size()
            .map(|(w, _)| w as usize)
            .unwrap_or_else(|_| 80);
        println!("{}", "─".repeat(width));
    }

    pub fn prompt_yes_no(question: &str) -> bool {
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

    pub fn prompt_input(prompt: &str) -> String {
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

    pub fn wait_for_enter() -> bool {
        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
            Ok(_) => true,
            Err(_) => false,
        }
    }
}
