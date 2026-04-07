use cursive::theme::{BaseColor, BorderStyle, Color, PaletteColor};
use serde::{Deserialize, Serialize};

// ── Default style file written on first run ───────────────────────────────────

const DEFAULT_STYLE_YAML: &str = "\
# ipatool TUI style configuration
#
# Colors: default, dark-black, dark-red, dark-green, dark-yellow, dark-blue,
#   dark-magenta, dark-cyan, dark-white, light-black, light-red, light-green,
#   light-yellow, light-blue, light-magenta, light-cyan, light-white
#
# Borders: none, simple, outset

shadow: false
borders: simple

# Selection highlight color (focused and unfocused panels)
highlight: dark-blue
highlight-inactive: dark-blue

# Terminal palette colors (\"default\" inherits the terminal's own colors)
background: default
view: default
primary: default
title-primary: default
";

// ── Color ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum StyleColor {
    Default,
    DarkBlack,
    DarkRed,
    DarkGreen,
    DarkYellow,
    DarkBlue,
    DarkMagenta,
    DarkCyan,
    DarkWhite,
    LightBlack,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    LightWhite,
}

impl StyleColor {
    pub fn to_cursive(&self) -> Color {
        match self {
            StyleColor::Default => Color::TerminalDefault,
            StyleColor::DarkBlack => Color::Dark(BaseColor::Black),
            StyleColor::DarkRed => Color::Dark(BaseColor::Red),
            StyleColor::DarkGreen => Color::Dark(BaseColor::Green),
            StyleColor::DarkYellow => Color::Dark(BaseColor::Yellow),
            StyleColor::DarkBlue => Color::Dark(BaseColor::Blue),
            StyleColor::DarkMagenta => Color::Dark(BaseColor::Magenta),
            StyleColor::DarkCyan => Color::Dark(BaseColor::Cyan),
            StyleColor::DarkWhite => Color::Dark(BaseColor::White),
            StyleColor::LightBlack => Color::Light(BaseColor::Black),
            StyleColor::LightRed => Color::Light(BaseColor::Red),
            StyleColor::LightGreen => Color::Light(BaseColor::Green),
            StyleColor::LightYellow => Color::Light(BaseColor::Yellow),
            StyleColor::LightBlue => Color::Light(BaseColor::Blue),
            StyleColor::LightMagenta => Color::Light(BaseColor::Magenta),
            StyleColor::LightCyan => Color::Light(BaseColor::Cyan),
            StyleColor::LightWhite => Color::Light(BaseColor::White),
        }
    }
}

// ── Borders ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum TuiBorders {
    None,
    Simple,
    Outset,
}

impl TuiBorders {
    pub fn to_cursive(&self) -> BorderStyle {
        match self {
            TuiBorders::None => BorderStyle::None,
            TuiBorders::Simple => BorderStyle::Simple,
            TuiBorders::Outset => BorderStyle::Outset,
        }
    }
}

// ── TuiStyle ──────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct TuiStyle {
    #[serde(default)]
    pub shadow: bool,
    #[serde(default = "default_borders")]
    pub borders: TuiBorders,
    #[serde(default = "default_highlight")]
    pub highlight: StyleColor,
    #[serde(default = "default_highlight")]
    pub highlight_inactive: StyleColor,
    #[serde(default = "default_terminal")]
    pub background: StyleColor,
    #[serde(default = "default_terminal")]
    pub view: StyleColor,
    #[serde(default = "default_terminal")]
    pub primary: StyleColor,
    #[serde(default = "default_terminal")]
    pub title_primary: StyleColor,
}

fn default_borders() -> TuiBorders {
    TuiBorders::Simple
}
fn default_highlight() -> StyleColor {
    StyleColor::DarkBlue
}
fn default_terminal() -> StyleColor {
    StyleColor::Default
}

impl Default for TuiStyle {
    fn default() -> Self {
        Self {
            shadow: false,
            borders: default_borders(),
            highlight: default_highlight(),
            highlight_inactive: default_highlight(),
            background: default_terminal(),
            view: default_terminal(),
            primary: default_terminal(),
            title_primary: default_terminal(),
        }
    }
}

impl TuiStyle {
    /// Load from a YAML file, or write the annotated default file and return
    /// defaults when the file does not exist.  Parse errors and permission
    /// errors are reported to stderr and fall back to defaults rather than
    /// crashing, since styling is non-critical.
    pub fn load_or_save_default(path: &str) -> Self {
        let expanded = crate::config::expand_path(path);
        match std::fs::read_to_string(&expanded) {
            Ok(content) => match serde_yaml::from_str::<TuiStyle>(&content) {
                Ok(style) => style,
                Err(e) => {
                    eprintln!(
                        "Warning: cannot parse TUI style file {}: {}; using defaults",
                        expanded.display(),
                        e
                    );
                    TuiStyle::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = expanded.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&expanded, DEFAULT_STYLE_YAML) {
                    eprintln!(
                        "Warning: cannot write default TUI style to {}: {}",
                        expanded.display(),
                        e
                    );
                }
                TuiStyle::default()
            }
            Err(e) => {
                eprintln!(
                    "Warning: cannot read TUI style file {}: {}; using defaults",
                    expanded.display(),
                    e
                );
                TuiStyle::default()
            }
        }
    }

    /// Apply this style to a cursive instance.
    pub fn apply(&self, siv: &mut cursive::Cursive) {
        use cursive::theme::Theme;
        let mut theme = Theme {
            shadow: self.shadow,
            borders: self.borders.to_cursive(),
            ..Theme::default()
        };
        theme.palette[PaletteColor::Background] = self.background.to_cursive();
        theme.palette[PaletteColor::View] = self.view.to_cursive();
        theme.palette[PaletteColor::Primary] = self.primary.to_cursive();
        theme.palette[PaletteColor::Secondary] = self.primary.to_cursive();
        theme.palette[PaletteColor::TitlePrimary] = self.title_primary.to_cursive();
        theme.palette[PaletteColor::TitleSecondary] = self.title_primary.to_cursive();
        theme.palette[PaletteColor::Highlight] = self.highlight.to_cursive();
        theme.palette[PaletteColor::HighlightInactive] = self.highlight_inactive.to_cursive();
        siv.set_theme(theme);
    }
}

/// Derive the style config path from the main config path.
/// `~/.ipa/toolconf.yaml` → `~/.ipa/toolconf-style.yaml`
pub fn style_config_path(config_path: &str) -> String {
    let base = config_path
        .strip_suffix(".yaml")
        .or_else(|| config_path.strip_suffix(".yml"))
        .unwrap_or(config_path);
    format!("{}-style.yaml", base)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_style_config_path_yaml() {
        assert_eq!(
            style_config_path("~/.ipa/toolconf.yaml"),
            "~/.ipa/toolconf-style.yaml"
        );
    }

    #[test]
    fn test_style_config_path_yml() {
        assert_eq!(
            style_config_path("~/.ipa/toolconf.yml"),
            "~/.ipa/toolconf-style.yaml"
        );
    }

    #[test]
    fn test_style_config_path_no_extension() {
        assert_eq!(
            style_config_path("~/.ipa/toolconf"),
            "~/.ipa/toolconf-style.yaml"
        );
    }

    #[test]
    fn test_default_yaml_parses() {
        // The embedded template must round-trip to the same values as TuiStyle::default().
        let parsed: TuiStyle = serde_yaml::from_str(DEFAULT_STYLE_YAML).unwrap();
        assert!(!parsed.shadow);
        assert_eq!(parsed.highlight, StyleColor::DarkBlue);
        assert_eq!(parsed.highlight_inactive, StyleColor::DarkBlue);
        assert_eq!(parsed.background, StyleColor::Default);
        assert_eq!(parsed.primary, StyleColor::Default);
    }

    #[test]
    fn test_style_color_roundtrip() {
        let colors = vec![
            StyleColor::Default,
            StyleColor::DarkBlue,
            StyleColor::LightGreen,
            StyleColor::DarkRed,
            StyleColor::LightYellow,
        ];
        for color in colors {
            let yaml = serde_yaml::to_string(&color).unwrap();
            let back: StyleColor = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(back, color);
        }
    }

    #[test]
    fn test_load_or_save_default_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.yaml");
        let style = TuiStyle::load_or_save_default(&path.to_string_lossy());
        assert!(!style.shadow);
        assert_eq!(style.highlight, StyleColor::DarkBlue);
        // The default file must have been written.
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("shadow:"));
        assert!(content.contains("highlight:"));
    }

    #[test]
    fn test_load_or_save_default_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.yaml");
        std::fs::write(
            &path,
            "shadow: true\nborders: outset\nhighlight: dark-red\n\
             highlight-inactive: dark-red\nbackground: default\n\
             view: default\nprimary: default\ntitle-primary: default\n",
        )
        .unwrap();
        let style = TuiStyle::load_or_save_default(&path.to_string_lossy());
        assert!(style.shadow);
        assert_eq!(style.highlight, StyleColor::DarkRed);
    }

    #[test]
    fn test_load_or_save_default_parse_error_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.yaml");
        std::fs::write(&path, "this: [broken yaml\n").unwrap();
        // Must not panic; returns defaults.
        let style = TuiStyle::load_or_save_default(&path.to_string_lossy());
        assert_eq!(style.highlight, StyleColor::DarkBlue);
    }
}
