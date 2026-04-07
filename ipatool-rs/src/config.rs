use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

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
