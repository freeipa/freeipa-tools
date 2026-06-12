use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

// ── PR source / issue tracker enums ──────────────────────────────────────────

/// Which forge hosts the pull requests that ipatool should operate on.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PrSource {
    /// GitHub (default)
    #[default]
    GitHub,
    /// Any Forgejo instance (includes Codeberg)
    Forgejo,
    /// Pagure
    Pagure,
}

/// Which issue tracker holds the tickets referenced by PRs.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum IssueTracker {
    /// Pagure (default)
    #[default]
    Pagure,
    /// Forgejo / Codeberg
    Forgejo,
    /// GitHub Issues
    GitHub,
}

// ── Per-profile overrides ─────────────────────────────────────────────────────

/// A named profile that overlays a subset of top-level config fields.
/// Only non-None fields are applied.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ProfileConfig {
    pub pr_source: Option<PrSource>,
    pub issue_tracker: Option<IssueTracker>,

    // GitHub overrides
    pub gh_token: Option<String>,
    pub gh_repo: Option<String>,
    pub gh_fork_remote: Option<String>,

    // Pagure overrides
    pub pagure_repository: Option<String>,
    pub pagure_token: Option<String>,

    // Forgejo overrides
    pub forgejo_url: Option<String>,
    pub forgejo_repo: Option<String>,
    pub forgejo_token: Option<String>,

    // URL overrides
    pub ticket_url: Option<String>,
    pub commit_url: Option<String>,
    pub db_path: Option<String>,

    // Migration overrides
    pub legacy_ticket_url: Option<String>,

    // Forgejo comment-field overrides
    /// Prefix for custom-field lines in Forgejo issue comments.
    /// Matching is case-sensitive. The prefix MUST end with `:` when non-empty;
    /// it is matched literally against the start of each comment line.
    /// Example: "ipatool:" → matches lines like "ipatool:rhbz: https://…".
    pub forgejo_comment_field_prefix: Option<String>,
}

// ── Main Config ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    #[serde(default)]
    pub clean_repo_path: String,
    #[serde(default = "default_remote")]
    pub remote: String,
    #[serde(default)]
    pub patchdir: String,

    #[serde(default)]
    pub ticket_url: String,
    #[serde(default)]
    pub commit_url: String,
    #[serde(default)]
    pub bugzilla_bug_url: String,
    #[serde(default)]
    pub jira_ticket_url: String,

    // Pagure
    #[serde(default)]
    pub pagure_repository: String,
    #[serde(default)]
    pub pagure_token: String,
    // Forgejo
    #[serde(default)]
    pub forgejo_url: String,
    #[serde(default)]
    pub forgejo_repo: String,
    #[serde(default)]
    pub forgejo_token: String,

    // Issue operations
    #[serde(default = "default_ask")]
    pub update_issue: String,
    #[serde(default = "default_ask")]
    pub close_issue: String,

    // Jira
    #[serde(default)]
    pub jira_token: String,
    #[serde(default = "default_ask")]
    pub update_jira: String,
    #[serde(default = "default_no")]
    pub close_jira: String,
    #[serde(default = "default_fixed")]
    pub jira_close_transition: String,

    // git am
    #[serde(default)]
    pub am_command: Vec<String>,

    // GitHub
    #[serde(default)]
    pub gh_token: String,
    #[serde(default)]
    pub gh_repo: String,
    #[serde(default)]
    pub gh_fork_remote: String,

    // Local cache DB
    #[serde(default = "default_db_path")]
    pub db_path: String,

    // Username map
    #[serde(default)]
    pub trac_username_map: HashMap<String, String>,

    // Named profiles (see --profile flag)
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,

    // ── Issue tracker migration support ───────────────────────────────────────
    /// Legacy ticket URL prefix to recognise in old commit messages.
    /// When set, commits referencing this prefix (e.g. https://pagure.io/freeipa/issue/)
    /// will have their issue numbers extracted in addition to those found via ticket-url.
    #[serde(default)]
    pub legacy_ticket_url: String,

    /// When true, legacy_ticket_url references in commit messages are rewritten
    /// to ticket_url before `git am` is applied, updating the history on push/backport.
    #[serde(default)]
    pub rewrite_ticket_urls: bool,

    /// Optional mapping from old (e.g. pagure) issue numbers to new (e.g. Codeberg)
    /// issue numbers when the migration did not preserve the original numbering.
    /// Numbers not present in the map are used as-is.
    #[serde(default)]
    pub issue_number_map: HashMap<u64, u64>,

    /// Line prefix used to identify custom-field lines in Forgejo issue comments.
    /// E.g. "ipatool:" → matches lines like "ipatool:rhbz: https://…".
    /// Empty string (default) matches bare "rhbz: …" / "reviewer: …" lines.
    /// Matching is case-sensitive. When non-empty, the prefix MUST end with `:`;
    /// it is matched literally against the start of each comment line.
    #[serde(default)]
    pub forgejo_comment_field_prefix: String,
}

fn default_remote() -> String {
    "origin".to_string()
}
fn default_ask() -> String {
    "ask".to_string()
}
fn default_no() -> String {
    "no".to_string()
}
fn default_fixed() -> String {
    "Fixed".to_string()
}
fn default_db_path() -> String {
    "~/.ipa/ipatool-cache.db".to_string()
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let expanded = expand_path(path);
        let content = std::fs::read_to_string(&expanded)
            .with_context(|| format!("Cannot read config file: {}", expanded.display()))?;
        let config: Config = serde_yaml::from_str(&content)
            .with_context(|| format!("Cannot parse config file: {}", expanded.display()))?;
        config.validate_forgejo_comment_field_prefix()?;
        Ok(config)
    }

    pub fn has_pagure(&self) -> bool {
        !self.pagure_token.is_empty() && !self.pagure_repository.is_empty()
    }

    pub fn has_forgejo(&self) -> bool {
        !self.forgejo_token.is_empty()
            && !self.forgejo_url.is_empty()
            && !self.forgejo_repo.is_empty()
    }

    pub fn has_jira(&self) -> bool {
        !self.jira_token.is_empty() && !self.jira_ticket_url.is_empty()
    }

    pub fn has_github(&self) -> bool {
        !self.gh_token.is_empty() && !self.gh_repo.is_empty()
    }

    pub fn jira_server(&self) -> Option<String> {
        if self.jira_ticket_url.contains("/browse/") {
            Some(
                self.jira_ticket_url
                    .split("/browse/")
                    .next()
                    .unwrap_or("")
                    .to_string(),
            )
        } else {
            None
        }
    }

    pub fn forgejo_owner_repo(&self) -> Result<(String, String)> {
        let parts: Vec<&str> = self.forgejo_repo.splitn(2, '/').collect();
        if parts.len() != 2 {
            bail!("forgejo-repo must be in 'owner/repo' format");
        }
        Ok((parts[0].to_string(), parts[1].to_string()))
    }

    pub fn gh_owner_repo(&self) -> Result<(String, String)> {
        let parts: Vec<&str> = self.gh_repo.splitn(2, '/').collect();
        if parts.len() != 2 {
            bail!("gh-repo must be in 'owner/repo' format");
        }
        Ok((parts[0].to_string(), parts[1].to_string()))
    }

    pub fn clean_repo_path_expanded(&self) -> PathBuf {
        expand_path(&self.clean_repo_path)
    }

    pub fn patchdir_expanded(&self) -> PathBuf {
        expand_path(&self.patchdir)
    }

    /// Apply a named profile's overrides to this config in-place.
    /// Returns an error if the profile name is not found.
    /// Profile field values replace corresponding top-level values when not None.
    pub fn apply_profile(&mut self, name: &str) -> Result<()> {
        let profile = self
            .profiles
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Profile '{}' not found in config", name))?;

        // Profile-overrideable fields (keep this list in sync with ProfileConfig):
        // - gh_token, gh_repo, gh_fork_remote
        // - pagure_repository, pagure_token
        // - forgejo_url, forgejo_repo, forgejo_token, forgejo_comment_field_prefix
        // - ticket_url, commit_url, legacy_ticket_url
        // - db_path
        // Note: pr_source and issue_tracker are handled separately (see NOTE below).
        // When adding a new ProfileConfig field, add its merge logic below AND update this list.

        // NOTE: `pr_source` and `issue_tracker` from the profile are intentionally
        // NOT merged into `self` here.  `Config` has no top-level `pr_source` /
        // `issue_tracker` fields; these values live only in `ProfileConfig` and are
        // read by `pr_source_for()` / `issue_tracker_for()` before `apply_profile`
        // is called.  The caller (build_ctx in main.rs) stores them in `Ctx` directly.
        // Merging them into `Config` would require adding new fields to `Config` and
        // would change the semantics of the existing helper methods, so the current
        // approach keeps things simple and consistent.

        if let Some(v) = profile.gh_token {
            self.gh_token = v;
        }
        if let Some(v) = profile.gh_repo {
            self.gh_repo = v;
        }
        if let Some(v) = profile.gh_fork_remote {
            self.gh_fork_remote = v;
        }
        if let Some(v) = profile.pagure_repository {
            self.pagure_repository = v;
        }
        if let Some(v) = profile.pagure_token {
            self.pagure_token = v;
        }
        if let Some(v) = profile.forgejo_url {
            self.forgejo_url = v;
        }
        if let Some(v) = profile.forgejo_repo {
            self.forgejo_repo = v;
        }
        if let Some(v) = profile.forgejo_token {
            self.forgejo_token = v;
        }
        if let Some(v) = profile.ticket_url {
            self.ticket_url = v;
        }
        if let Some(v) = profile.commit_url {
            self.commit_url = v;
        }
        if let Some(v) = profile.db_path {
            self.db_path = v;
        }
        if let Some(v) = profile.legacy_ticket_url {
            self.legacy_ticket_url = v;
        }
        if let Some(v) = profile.forgejo_comment_field_prefix {
            self.forgejo_comment_field_prefix = v;
        }

        self.validate_forgejo_comment_field_prefix()?;
        Ok(())
    }

    /// Validate config invariants that cannot be enforced by serde alone.
    /// Returns an error if a misconfigured field is detected.
    fn validate_forgejo_comment_field_prefix(&self) -> Result<()> {
        if !self.forgejo_comment_field_prefix.is_empty()
            && !self.forgejo_comment_field_prefix.ends_with(':')
        {
            bail!(
                "forgejo-comment-field-prefix must end with ':' (e.g. \"ipatool:\"); \
                 the colon separates the prefix from the field name. Got: {:?}",
                self.forgejo_comment_field_prefix
            );
        }
        Ok(())
    }

    /// Return the effective PR source for a profile (if named) or the default.
    pub fn pr_source_for(&self, profile: Option<&str>) -> PrSource {
        profile
            .and_then(|n| self.profiles.get(n))
            .and_then(|p| p.pr_source.clone())
            .unwrap_or_default()
    }

    /// Return the effective issue tracker for a profile (if named) or the default.
    pub fn issue_tracker_for(&self, profile: Option<&str>) -> IssueTracker {
        profile
            .and_then(|n| self.profiles.get(n))
            .and_then(|p| p.issue_tracker.clone())
            .unwrap_or_default()
    }

    pub fn sanitized_display(&self) -> String {
        let mut lines = vec![];
        lines.push(format!("clean-repo-path: {}", self.clean_repo_path));
        lines.push(format!("remote: {}", self.remote));
        lines.push(format!("patchdir: {}", self.patchdir));
        lines.push(format!("ticket-url: {}", self.ticket_url));
        if !self.pagure_repository.is_empty() {
            lines.push(format!("pagure-repository: {}", self.pagure_repository));
            lines.push("pagure-token: ***".to_string());
        }
        if !self.forgejo_url.is_empty() {
            lines.push(format!("forgejo-url: {}", self.forgejo_url));
            lines.push(format!("forgejo-repo: {}", self.forgejo_repo));
            lines.push("forgejo-token: ***".to_string());
        }
        if !self.jira_token.is_empty() {
            lines.push("jira-token: ***".to_string());
        }
        if !self.gh_token.is_empty() {
            lines.push(format!("gh-repo: {}", self.gh_repo));
            lines.push("gh-token: ***".to_string());
        }
        lines.join("\n")
    }
}

pub fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_default_is_empty_strings() {
        // Config::default() is the Rust-derived Default, giving "" for String fields.
        // The serde defaults ("origin", "ask", etc.) only apply during deserialization.
        let config = Config::default();
        assert!(config.pagure_token.is_empty());
        assert!(config.gh_token.is_empty());
        assert!(config.jira_token.is_empty());
        assert!(config.trac_username_map.is_empty());
    }

    #[test]
    fn test_serde_defaults_on_empty_yaml() {
        // When deserializing minimal YAML, serde-level defaults kick in
        let config: Config = serde_yaml::from_str("{}").unwrap();
        assert_eq!(config.remote, "origin");
        assert_eq!(config.update_issue, "ask");
        assert_eq!(config.close_issue, "ask");
        assert_eq!(config.update_jira, "ask");
        assert_eq!(config.close_jira, "no");
        assert_eq!(config.jira_close_transition, "Fixed");
        assert_eq!(config.db_path, "~/.ipa/ipatool-cache.db");
    }

    #[test]
    fn test_has_pagure_true() {
        let mut c = Config::default();
        c.pagure_token = "tok".to_string();
        c.pagure_repository = "repo".to_string();
        assert!(c.has_pagure());
    }

    #[test]
    fn test_has_pagure_false_missing_token() {
        let mut c = Config::default();
        c.pagure_repository = "repo".to_string();
        assert!(!c.has_pagure());
    }

    #[test]
    fn test_has_pagure_false_missing_repo() {
        let mut c = Config::default();
        c.pagure_token = "tok".to_string();
        assert!(!c.has_pagure());
    }

    #[test]
    fn test_has_forgejo_true() {
        let mut c = Config::default();
        c.forgejo_token = "tok".to_string();
        c.forgejo_url = "https://forgejo.example.com".to_string();
        c.forgejo_repo = "owner/repo".to_string();
        assert!(c.has_forgejo());
    }

    #[test]
    fn test_has_forgejo_false_partial() {
        let mut c = Config::default();
        c.forgejo_token = "tok".to_string();
        // missing url and repo
        assert!(!c.has_forgejo());
    }

    #[test]
    fn test_has_jira_true() {
        let mut c = Config::default();
        c.jira_token = "tok".to_string();
        c.jira_ticket_url = "https://issues.redhat.com/browse/RHEL-".to_string();
        assert!(c.has_jira());
    }

    #[test]
    fn test_has_jira_false_no_url() {
        let mut c = Config::default();
        c.jira_token = "tok".to_string();
        assert!(!c.has_jira());
    }

    #[test]
    fn test_has_github_true() {
        let mut c = Config::default();
        c.gh_token = "tok".to_string();
        c.gh_repo = "owner/repo".to_string();
        assert!(c.has_github());
    }

    #[test]
    fn test_has_github_false_no_repo() {
        let mut c = Config::default();
        c.gh_token = "tok".to_string();
        assert!(!c.has_github());
    }

    #[test]
    fn test_jira_server_with_browse() {
        let mut c = Config::default();
        c.jira_ticket_url = "https://issues.redhat.com/browse/RHEL-".to_string();
        assert_eq!(
            c.jira_server(),
            Some("https://issues.redhat.com".to_string())
        );
    }

    #[test]
    fn test_jira_server_without_browse() {
        let mut c = Config::default();
        c.jira_ticket_url = "https://issues.redhat.com/ticket/".to_string();
        assert_eq!(c.jira_server(), None);
    }

    #[test]
    fn test_jira_server_empty() {
        let c = Config::default();
        assert_eq!(c.jira_server(), None);
    }

    #[test]
    fn test_gh_owner_repo_valid() {
        let mut c = Config::default();
        c.gh_repo = "freeipa/freeipa".to_string();
        let (owner, repo) = c.gh_owner_repo().unwrap();
        assert_eq!(owner, "freeipa");
        assert_eq!(repo, "freeipa");
    }

    #[test]
    fn test_gh_owner_repo_invalid() {
        let mut c = Config::default();
        c.gh_repo = "noslash".to_string();
        assert!(c.gh_owner_repo().is_err());
    }

    #[test]
    fn test_forgejo_owner_repo_valid() {
        let mut c = Config::default();
        c.forgejo_repo = "owner/project".to_string();
        let (owner, repo) = c.forgejo_owner_repo().unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "project");
    }

    #[test]
    fn test_forgejo_owner_repo_with_extra_slash() {
        // splitn(2) means only first slash is split on
        let mut c = Config::default();
        c.forgejo_repo = "owner/repo/extra".to_string();
        let (owner, repo) = c.forgejo_owner_repo().unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo/extra");
    }

    #[test]
    fn test_forgejo_owner_repo_invalid() {
        let mut c = Config::default();
        c.forgejo_repo = "noslash".to_string();
        assert!(c.forgejo_owner_repo().is_err());
    }

    #[test]
    fn test_expand_path_tilde() {
        let p = expand_path("~/foo/bar");
        let home = dirs::home_dir().unwrap();
        assert_eq!(p, home.join("foo/bar"));
    }

    #[test]
    fn test_expand_path_absolute() {
        let p = expand_path("/absolute/path");
        assert_eq!(p, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_expand_path_relative() {
        let p = expand_path("relative/path");
        assert_eq!(p, PathBuf::from("relative/path"));
    }

    #[test]
    fn test_expand_path_tilde_only() {
        // "~" alone (no slash) should NOT be expanded — only "~/"
        let p = expand_path("~");
        assert_eq!(p, PathBuf::from("~"));
    }

    #[test]
    fn test_yaml_parsing_basic() {
        let yaml = "
clean-repo-path: ~/dev/freeipa
remote: upstream
pagure-token: mytoken
pagure-repository: freeipa
gh-token: ghtoken
gh-repo: freeipa/freeipa
update-issue: yes
close-issue: no
";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.clean_repo_path, "~/dev/freeipa");
        assert_eq!(config.remote, "upstream");
        assert_eq!(config.pagure_token, "mytoken");
        assert_eq!(config.pagure_repository, "freeipa");
        assert_eq!(config.gh_token, "ghtoken");
        assert_eq!(config.gh_repo, "freeipa/freeipa");
        assert_eq!(config.update_issue, "yes");
        assert_eq!(config.close_issue, "no");
    }

    #[test]
    fn test_yaml_parsing_defaults_when_absent() {
        // Minimal YAML: unset keys should get their defaults
        let yaml = "clean-repo-path: ~/dev/freeipa\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.remote, "origin");
        assert_eq!(config.update_issue, "ask");
        assert_eq!(config.close_jira, "no");
    }

    // ── Profile tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_pr_source_default() {
        assert_eq!(PrSource::default(), PrSource::GitHub);
    }

    #[test]
    fn test_issue_tracker_default() {
        assert_eq!(IssueTracker::default(), IssueTracker::Pagure);
    }

    #[test]
    fn test_pr_source_for_no_profile() {
        let config: Config = serde_yaml::from_str("{}").unwrap();
        assert_eq!(config.pr_source_for(None), PrSource::GitHub);
    }

    #[test]
    fn test_issue_tracker_for_no_profile() {
        let config: Config = serde_yaml::from_str("{}").unwrap();
        assert_eq!(config.issue_tracker_for(None), IssueTracker::Pagure);
    }

    #[test]
    fn test_profiles_parsed_from_yaml() {
        let yaml = r#"
profiles:
  codeberg:
    pr-source: forgejo
    issue-tracker: forgejo
    forgejo-url: https://codeberg.org
    forgejo-repo: myuser/freeipa
    forgejo-token: "mytoken"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.profiles.contains_key("codeberg"));
        let p = &config.profiles["codeberg"];
        assert_eq!(p.pr_source, Some(PrSource::Forgejo));
        assert_eq!(p.issue_tracker, Some(IssueTracker::Forgejo));
        assert_eq!(p.forgejo_url.as_deref(), Some("https://codeberg.org"));
        assert_eq!(p.forgejo_repo.as_deref(), Some("myuser/freeipa"));
        assert_eq!(p.forgejo_token.as_deref(), Some("mytoken"));
    }

    #[test]
    fn test_pr_source_for_named_profile() {
        let yaml = r#"
profiles:
  codeberg:
    pr-source: forgejo
    issue-tracker: forgejo
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.pr_source_for(Some("codeberg")), PrSource::Forgejo);
        assert_eq!(
            config.issue_tracker_for(Some("codeberg")),
            IssueTracker::Forgejo
        );
    }

    #[test]
    fn test_pr_source_for_unknown_profile_returns_default() {
        let config: Config = serde_yaml::from_str("{}").unwrap();
        assert_eq!(config.pr_source_for(Some("nonexistent")), PrSource::GitHub);
        assert_eq!(
            config.issue_tracker_for(Some("nonexistent")),
            IssueTracker::Pagure
        );
    }

    #[test]
    fn test_apply_profile_overlays_fields() {
        let yaml = r#"
pagure-repository: freeipa
pagure-token: orig-pagure-token
profiles:
  alt:
    pagure-repository: freeipa-alt
    pagure-token: alt-pagure-token
    ticket-url: https://alt.example.com/issue/
"#;
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.pagure_repository, "freeipa");
        config.apply_profile("alt").unwrap();
        assert_eq!(config.pagure_repository, "freeipa-alt");
        assert_eq!(config.pagure_token, "alt-pagure-token");
        assert_eq!(config.ticket_url, "https://alt.example.com/issue/");
    }

    #[test]
    fn test_apply_profile_error_on_missing() {
        let mut config: Config = serde_yaml::from_str("{}").unwrap();
        assert!(config.apply_profile("nonexistent").is_err());
    }

    #[test]
    fn test_apply_profile_leaves_unset_fields_unchanged() {
        let yaml = r#"
pagure-repository: freeipa
pagure-token: mytoken
forgejo-url: https://example.com
forgejo-repo: owner/repo
forgejo-token: ftoken
profiles:
  partial:
    pr-source: forgejo
"#;
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        config.apply_profile("partial").unwrap();
        // Fields not in profile are unchanged
        assert_eq!(config.pagure_repository, "freeipa");
        assert_eq!(config.pagure_token, "mytoken");
        assert_eq!(config.forgejo_url, "https://example.com");
    }

    #[test]
    fn test_profile_pr_source_github_parseable() {
        let yaml = "profiles:\n  x:\n    pr-source: github\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.profiles["x"].pr_source, Some(PrSource::GitHub));
    }

    #[test]
    fn test_profile_issue_tracker_github_parseable() {
        let yaml = "profiles:\n  x:\n    issue-tracker: github\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.profiles["x"].issue_tracker,
            Some(IssueTracker::GitHub)
        );
    }

    #[test]
    fn test_profile_issue_tracker_pagure_parseable() {
        let yaml = "profiles:\n  x:\n    issue-tracker: pagure\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.profiles["x"].issue_tracker,
            Some(IssueTracker::Pagure)
        );
    }

    #[test]
    fn test_forgejo_comment_field_prefix_colon_required_when_non_empty() {
        // "ipatool:" (with trailing colon) is the documented valid form
        let mut c = Config::default();
        c.forgejo_comment_field_prefix = "ipatool:".to_string();
        assert!(c.validate_forgejo_comment_field_prefix().is_ok());
    }

    #[test]
    fn test_forgejo_comment_field_prefix_no_colon_rejected() {
        // "ipatool" without trailing colon must be rejected
        let mut c = Config::default();
        c.forgejo_comment_field_prefix = "ipatool".to_string();
        assert!(c.validate_forgejo_comment_field_prefix().is_err());
    }

    #[test]
    fn test_forgejo_comment_field_prefix_empty_valid() {
        // empty prefix is always valid (no colon required)
        let c = Config::default();
        assert!(c.validate_forgejo_comment_field_prefix().is_ok());
    }

    #[test]
    fn test_apply_profile_rejects_bad_comment_field_prefix() {
        // Profile value without trailing colon must be rejected
        let yaml = r#"
profiles:
  bad:
    forgejo-comment-field-prefix: "ipatool"
"#;
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.apply_profile("bad").is_err());
    }

    #[test]
    fn test_apply_profile_accepts_valid_comment_field_prefix() {
        // Profile value with trailing colon must be accepted
        let yaml = r#"
profiles:
  good:
    forgejo-comment-field-prefix: "ipatool:"
"#;
        let mut config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.apply_profile("good").is_ok());
        assert_eq!(config.forgejo_comment_field_prefix, "ipatool:");
    }

    #[test]
    fn test_yaml_parsing_trac_username_map() {
        let yaml = "
trac-username-map:
  abbra: Alexander Bokovoy <abokovoy@redhat.com>
  simo: Simo Sorce <ssorce@redhat.com>
";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.trac_username_map.get("abbra").map(|s| s.as_str()),
            Some("Alexander Bokovoy <abokovoy@redhat.com>")
        );
        assert_eq!(config.trac_username_map.len(), 2);
    }
}
