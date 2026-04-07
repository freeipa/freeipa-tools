use serde::{Deserialize, Serialize};

// ── Default key file written on first run ─────────────────────────────────────

const DEFAULT_KEYS_YAML: &str = "\
# ipatool TUI keybindings configuration
#
# Each value must be a single character.
# Changes take effect on the next startup.
#
# Note: some keys have the same default in different views (e.g. 'c' is
# \"review\" in the PR list and \"comment\" in the diff view).  These are on
# separate layers, so they never conflict.

# ── Main PR list ──────────────────────────────────────────────────────────────
quit:        q
ack:         a
reject:      x
browser:     b
refresh:     r
review:      c
sync:        s
down:        j
up:          k
scroll-down: d
scroll-up:   u

# ── Code review diff view ─────────────────────────────────────────────────────
review-comment: c
review-down:    j
review-up:      k
review-close:   q

# ── Action dialog ─────────────────────────────────────────────────────────────
action-review:   v
action-labels:   l
action-push:     p
action-backport: B
";

// ── TuiKeys ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case", default)]
pub struct TuiKeys {
    // Main two-pane view
    pub quit:        char,
    pub ack:         char,
    pub reject:      char,
    pub browser:     char,
    pub refresh:     char,
    pub review:      char,
    pub sync:        char,
    pub down:        char,
    pub up:          char,
    pub scroll_down: char,
    pub scroll_up:   char,
    // Code review diff view
    pub review_comment: char,
    pub review_down:    char,
    pub review_up:      char,
    pub review_close:   char,
    // Action dialog
    pub action_review:   char,
    pub action_labels:   char,
    pub action_push:     char,
    pub action_backport: char,
}

impl Default for TuiKeys {
    fn default() -> Self {
        Self {
            quit:        'q',
            ack:         'a',
            reject:      'x',
            browser:     'b',
            refresh:     'r',
            review:      'c',
            sync:        's',
            down:        'j',
            up:          'k',
            scroll_down: 'd',
            scroll_up:   'u',
            review_comment: 'c',
            review_down:    'j',
            review_up:      'k',
            review_close:   'q',
            action_review:   'v',
            action_labels:   'l',
            action_push:     'p',
            action_backport: 'B',
        }
    }
}

impl TuiKeys {
    /// Load from a YAML file, or write the annotated default file and return
    /// defaults when the file does not exist.  Parse errors and permission
    /// errors are reported to stderr and fall back to defaults rather than
    /// crashing, since keybindings are non-critical.
    pub fn load_or_save_default(path: &str) -> Self {
        let expanded = crate::config::expand_path(path);
        match std::fs::read_to_string(&expanded) {
            Ok(content) => match serde_yaml::from_str::<TuiKeys>(&content) {
                Ok(keys) => keys,
                Err(e) => {
                    eprintln!(
                        "Warning: cannot parse TUI keybindings file {}: {}; using defaults",
                        expanded.display(),
                        e
                    );
                    TuiKeys::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = expanded.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&expanded, DEFAULT_KEYS_YAML) {
                    eprintln!(
                        "Warning: cannot write default TUI keybindings to {}: {}",
                        expanded.display(),
                        e
                    );
                }
                TuiKeys::default()
            }
            Err(e) => {
                eprintln!(
                    "Warning: cannot read TUI keybindings file {}: {}; using defaults",
                    expanded.display(),
                    e
                );
                TuiKeys::default()
            }
        }
    }
}

/// Derive the keybindings config path from the main config path.
/// `~/.ipa/toolconf.yaml` → `~/.ipa/toolconf-keys.yaml`
pub fn keys_config_path(config_path: &str) -> String {
    let base = config_path
        .strip_suffix(".yaml")
        .or_else(|| config_path.strip_suffix(".yml"))
        .unwrap_or(config_path);
    format!("{}-keys.yaml", base)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keys_config_path_yaml() {
        assert_eq!(
            keys_config_path("~/.ipa/toolconf.yaml"),
            "~/.ipa/toolconf-keys.yaml"
        );
    }

    #[test]
    fn test_keys_config_path_yml() {
        assert_eq!(
            keys_config_path("~/.ipa/toolconf.yml"),
            "~/.ipa/toolconf-keys.yaml"
        );
    }

    #[test]
    fn test_keys_config_path_no_extension() {
        assert_eq!(
            keys_config_path("~/.ipa/toolconf"),
            "~/.ipa/toolconf-keys.yaml"
        );
    }

    #[test]
    fn test_default_yaml_parses() {
        let parsed: TuiKeys = serde_yaml::from_str(DEFAULT_KEYS_YAML).unwrap();
        let defaults = TuiKeys::default();
        assert_eq!(parsed.quit, defaults.quit);
        assert_eq!(parsed.ack, defaults.ack);
        assert_eq!(parsed.reject, defaults.reject);
        assert_eq!(parsed.browser, defaults.browser);
        assert_eq!(parsed.refresh, defaults.refresh);
        assert_eq!(parsed.review, defaults.review);
        assert_eq!(parsed.sync, defaults.sync);
        assert_eq!(parsed.down, defaults.down);
        assert_eq!(parsed.up, defaults.up);
        assert_eq!(parsed.scroll_down, defaults.scroll_down);
        assert_eq!(parsed.scroll_up, defaults.scroll_up);
        assert_eq!(parsed.review_comment, defaults.review_comment);
        assert_eq!(parsed.review_down, defaults.review_down);
        assert_eq!(parsed.review_up, defaults.review_up);
        assert_eq!(parsed.review_close, defaults.review_close);
        assert_eq!(parsed.action_review, defaults.action_review);
        assert_eq!(parsed.action_labels, defaults.action_labels);
        assert_eq!(parsed.action_push, defaults.action_push);
        assert_eq!(parsed.action_backport, defaults.action_backport);
    }

    #[test]
    fn test_load_or_save_default_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.yaml");
        let keys = TuiKeys::load_or_save_default(&path.to_string_lossy());
        assert_eq!(keys.quit, 'q');
        assert_eq!(keys.ack, 'a');
        assert_eq!(keys.action_backport, 'B');
        // The default file must have been written.
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("ack:"));
        assert!(content.contains("quit:"));
    }

    #[test]
    fn test_load_or_save_default_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.yaml");
        std::fs::write(
            &path,
            "quit: Q\nack: A\nreject: X\nbrowser: b\nrefresh: r\nreview: c\n\
             sync: s\ndown: j\nup: k\nscroll-down: d\nscroll-up: u\n\
             review-comment: c\nreview-down: j\nreview-up: k\nreview-close: Q\n\
             action-review: v\naction-labels: l\naction-push: p\naction-backport: b\n",
        )
        .unwrap();
        let keys = TuiKeys::load_or_save_default(&path.to_string_lossy());
        assert_eq!(keys.quit, 'Q');
        assert_eq!(keys.ack, 'A');
        assert_eq!(keys.action_backport, 'b');
    }

    #[test]
    fn test_load_or_save_default_parse_error_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.yaml");
        std::fs::write(&path, "this: [broken yaml\n").unwrap();
        // Must not panic; returns defaults.
        let keys = TuiKeys::load_or_save_default(&path.to_string_lossy());
        assert_eq!(keys.quit, 'q');
        assert_eq!(keys.ack, 'a');
    }
}
