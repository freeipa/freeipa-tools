use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "always" => ColorMode::Always,
            "never" => ColorMode::Never,
            _ => ColorMode::Auto,
        }
    }

    pub fn enabled(&self) -> bool {
        match self {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => atty_stdout(),
        }
    }
}

fn atty_stdout() -> bool {
    use std::io::IsTerminal;
    io::stdout().is_terminal()
}

pub struct Output {
    pub color: ColorMode,
}

impl Output {
    pub fn new(color: ColorMode) -> Self {
        Output { color }
    }

    pub fn print_colored(&self, msg: &str, color: Color) {
        if self.color.enabled() {
            println!("{}{}{}", color.ansi_code(), msg, RESET);
        } else {
            println!("{}", msg);
        }
    }

    pub fn print_red(&self, msg: &str) {
        self.print_colored(msg, Color::Red);
    }

    pub fn print_green(&self, msg: &str) {
        self.print_colored(msg, Color::Green);
    }

    pub fn print_yellow(&self, msg: &str) {
        self.print_colored(msg, Color::Yellow);
    }

    #[allow(dead_code)]
    pub fn print_blue(&self, msg: &str) {
        self.print_colored(msg, Color::Blue);
    }

    pub fn print_cyan(&self, msg: &str) {
        self.print_colored(msg, Color::Cyan);
    }

    pub fn section(&self, title: &str) {
        self.print_cyan(&format!("=== {} ===", title));
    }
}

const RESET: &str = "\x1b[0m";

pub enum Color {
    Red,
    Green,
    Yellow,
    #[allow(dead_code)]
    Blue,
    Cyan,
    Rgb(u8, u8, u8),
}

impl Color {
    pub fn ansi_code(&self) -> String {
        match self {
            Color::Red => "\x1b[31m".to_string(),
            Color::Green => "\x1b[32m".to_string(),
            Color::Yellow => "\x1b[33m".to_string(),
            Color::Blue => "\x1b[34m".to_string(),
            Color::Cyan => "\x1b[36m".to_string(),
            Color::Rgb(r, g, b) => format!("\x1b[38;2;{};{};{}m", r, g, b),
        }
    }
}

pub fn prompt(msg: &str) -> String {
    print!("{}", msg);
    io::stdout().flush().unwrap();
    let mut line = String::new();
    io::stdin().read_line(&mut line).unwrap();
    line.trim().to_string()
}

pub fn ask_yn(config_val: &str, config_key: &str, prompt_msg: &str) -> bool {
    match config_val {
        "no" => false,
        "yes" => true,
        "ask" => {
            let response = prompt(&format!("{} [y/n] ", prompt_msg));
            response.to_lowercase() == "y"
        }
        _ => {
            eprintln!("Invalid value for \"{}\" in config file", config_key);
            let response = prompt(&format!("{} [y/n] ", prompt_msg));
            response.to_lowercase() == "y"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Color::ansi_code ──────────────────────────────────────────────────────

    #[test]
    fn test_color_red() {
        assert_eq!(Color::Red.ansi_code(), "\x1b[31m");
    }

    #[test]
    fn test_color_green() {
        assert_eq!(Color::Green.ansi_code(), "\x1b[32m");
    }

    #[test]
    fn test_color_yellow() {
        assert_eq!(Color::Yellow.ansi_code(), "\x1b[33m");
    }

    #[test]
    fn test_color_blue() {
        assert_eq!(Color::Blue.ansi_code(), "\x1b[34m");
    }

    #[test]
    fn test_color_cyan() {
        assert_eq!(Color::Cyan.ansi_code(), "\x1b[36m");
    }

    #[test]
    fn test_color_rgb() {
        assert_eq!(Color::Rgb(255, 51, 17).ansi_code(), "\x1b[38;2;255;51;17m");
    }

    #[test]
    fn test_color_rgb_zeros() {
        assert_eq!(Color::Rgb(0, 0, 0).ansi_code(), "\x1b[38;2;0;0;0m");
    }

    // ── ColorMode ─────────────────────────────────────────────────────────────

    #[test]
    fn test_color_mode_always() {
        assert_eq!(ColorMode::from_str("always"), ColorMode::Always);
    }

    #[test]
    fn test_color_mode_never() {
        assert_eq!(ColorMode::from_str("never"), ColorMode::Never);
    }

    #[test]
    fn test_color_mode_auto() {
        assert_eq!(ColorMode::from_str("auto"), ColorMode::Auto);
    }

    #[test]
    fn test_color_mode_unknown_defaults_auto() {
        assert_eq!(ColorMode::from_str("bogus"), ColorMode::Auto);
        assert_eq!(ColorMode::from_str(""), ColorMode::Auto);
        assert_eq!(ColorMode::from_str("ALWAYS"), ColorMode::Auto);
    }

    #[test]
    fn test_color_mode_always_enabled() {
        assert!(ColorMode::Always.enabled());
    }

    #[test]
    fn test_color_mode_never_enabled() {
        assert!(!ColorMode::Never.enabled());
    }

    // ── ask_yn ────────────────────────────────────────────────────────────────

    #[test]
    fn test_ask_yn_yes() {
        assert!(ask_yn("yes", "key", "prompt"));
    }

    #[test]
    fn test_ask_yn_no() {
        assert!(!ask_yn("no", "key", "prompt"));
    }
}
