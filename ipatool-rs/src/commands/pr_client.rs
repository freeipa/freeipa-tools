/// Unified pull-request client that abstracts over GitHub and Forgejo.
///
/// Each variant wraps the underlying provider's client.  All methods return the
/// shared GitHub-shaped types (`GitHubPR`, `GitHubCommit`, …) so the rest of
/// the codebase does not need to know which forge is in use.
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use crate::api::forgejo::ForgejoClient;
use crate::api::github::{
    CiJobStatus, GitHubClient, GitHubComment, GitHubFile, GitHubLabel, GitHubPR,
    GitHubReviewComment,
};

pub enum PrClient {
    GitHub(Arc<GitHubClient>),
    Forgejo(Arc<ForgejoClient>),
}

impl PrClient {
    // ── PR listing ────────────────────────────────────────────────────────────

    pub fn list_prs(&self, state: &str) -> Result<Vec<GitHubPR>> {
        match self {
            PrClient::GitHub(gh) => gh.list_prs(state),
            PrClient::Forgejo(fj) => fj.list_prs(state),
        }
    }

    pub fn list_prs_limited(
        &self,
        state: &str,
        limit: usize,
        on_page: impl FnMut(u32, usize),
    ) -> Result<Vec<GitHubPR>> {
        match self {
            PrClient::GitHub(gh) => gh.list_prs_limited(state, limit, on_page),
            PrClient::Forgejo(fj) => fj.list_prs_limited(state, limit, on_page),
        }
    }

    // ── Single PR ─────────────────────────────────────────────────────────────

    pub fn get_pr(&self, number: u64) -> Result<GitHubPR> {
        match self {
            PrClient::GitHub(gh) => gh.get_pr(number),
            PrClient::Forgejo(fj) => fj.get_pr(number),
        }
    }

    pub fn is_pr_merged(&self, number: u64) -> Result<bool> {
        match self {
            PrClient::GitHub(gh) => gh.is_pr_merged(number),
            PrClient::Forgejo(fj) => fj.is_pr_merged(number),
        }
    }

    // ── Labels ────────────────────────────────────────────────────────────────

    /// Return the current labels on a PR/issue (as GitHubLabel list).
    pub fn get_pr_labels(&self, number: u64) -> Result<Vec<GitHubLabel>> {
        match self {
            PrClient::GitHub(gh) => {
                let issue = gh.get_issue(number)?;
                Ok(issue.labels)
            }
            PrClient::Forgejo(fj) => {
                let lbls = fj.get_issue_labels_with_id(number)?;
                Ok(lbls
                    .into_iter()
                    .map(|l| GitHubLabel {
                        name: l.name,
                        color: l.color,
                    })
                    .collect())
            }
        }
    }

    pub fn add_labels(&self, number: u64, labels: &[&str]) -> Result<()> {
        match self {
            PrClient::GitHub(gh) => gh.add_labels(number, labels),
            PrClient::Forgejo(fj) => fj.add_labels(number, labels),
        }
    }

    pub fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        match self {
            PrClient::GitHub(gh) => gh.remove_label(number, label),
            PrClient::Forgejo(fj) => fj.remove_label(number, label),
        }
    }

    pub fn list_repo_labels(&self) -> Result<Vec<GitHubLabel>> {
        match self {
            PrClient::GitHub(gh) => gh.list_repo_labels(),
            PrClient::Forgejo(fj) => fj.list_repo_labels(),
        }
    }

    // ── Comments ──────────────────────────────────────────────────────────────

    pub fn create_comment(&self, number: u64, text: &str) -> Result<()> {
        match self {
            PrClient::GitHub(gh) => gh.create_comment(number, text),
            PrClient::Forgejo(fj) => fj.comment_issue(number, text),
        }
    }

    pub fn get_last_issue_comments(&self, number: u64, n: usize) -> Result<Vec<GitHubComment>> {
        match self {
            PrClient::GitHub(gh) => gh.get_last_issue_comments(number, n),
            PrClient::Forgejo(fj) => fj.get_last_issue_comments(number, n),
        }
    }

    pub fn get_all_issue_comments(&self, number: u64) -> Result<Vec<GitHubComment>> {
        match self {
            PrClient::GitHub(gh) => gh.get_all_issue_comments(number),
            PrClient::Forgejo(fj) => fj.get_all_issue_comments(number),
        }
    }

    // ── Review comments ───────────────────────────────────────────────────────

    pub fn list_review_comments(&self, pr_number: u64) -> Result<Vec<GitHubReviewComment>> {
        match self {
            PrClient::GitHub(gh) => gh.list_review_comments(pr_number),
            PrClient::Forgejo(fj) => fj.list_review_comments(pr_number),
        }
    }

    pub fn create_review_comment(
        &self,
        pr_number: u64,
        commit_id: &str,
        path: &str,
        line: u64,
        body: &str,
    ) -> Result<()> {
        match self {
            PrClient::GitHub(gh) => {
                gh.create_review_comment(pr_number, commit_id, path, line, body)
            }
            PrClient::Forgejo(fj) => {
                fj.create_review_comment(pr_number, commit_id, path, line, body)
            }
        }
    }

    // ── Close ─────────────────────────────────────────────────────────────────

    /// Close a PR (and the underlying issue on GitHub).
    pub fn close_pr(&self, number: u64) -> Result<()> {
        match self {
            PrClient::GitHub(gh) => gh.close_issue(number),
            PrClient::Forgejo(fj) => fj.close_pr(number),
        }
    }

    // ── Commits & patches ─────────────────────────────────────────────────────

    pub fn get_pr_commits(&self, number: u64) -> Result<Vec<crate::api::github::GitHubCommit>> {
        match self {
            PrClient::GitHub(gh) => gh.get_pr_commits(number),
            PrClient::Forgejo(fj) => fj.get_pr_commits(number),
        }
    }

    pub fn get_commit_patch(&self, sha: &str) -> Result<Vec<u8>> {
        match self {
            PrClient::GitHub(gh) => gh.get_commit_patch(sha),
            PrClient::Forgejo(fj) => fj.get_commit_patch(sha),
        }
    }

    // ── CI statuses ───────────────────────────────────────────────────────────

    pub fn most_recent_statuses(&self, sha: &str) -> Result<HashMap<String, CiJobStatus>> {
        match self {
            PrClient::GitHub(gh) => gh.most_recent_statuses(sha),
            PrClient::Forgejo(fj) => fj.most_recent_statuses(sha),
        }
    }

    // ── Files ─────────────────────────────────────────────────────────────────

    pub fn get_pr_files(&self, number: u64) -> Result<Vec<GitHubFile>> {
        match self {
            PrClient::GitHub(gh) => gh.get_pr_files(number),
            PrClient::Forgejo(fj) => fj.get_pr_files(number),
        }
    }

    // ── User ──────────────────────────────────────────────────────────────────

    pub fn get_authenticated_user_login(&self) -> Result<String> {
        match self {
            PrClient::GitHub(gh) => gh.get_authenticated_user_login(),
            PrClient::Forgejo(fj) => fj.get_authenticated_user_login(),
        }
    }

    // ── PR creation ───────────────────────────────────────────────────────────

    pub fn create_pr(&self, title: &str, base: &str, head: &str, body: &str) -> Result<GitHubPR> {
        match self {
            PrClient::GitHub(gh) => gh.create_pr(title, base, head, body),
            PrClient::Forgejo(fj) => fj.create_pr(title, base, head, body),
        }
    }

    // ── Provider identity ─────────────────────────────────────────────────────

    /// Return the `Provider` variant that corresponds to this client, for use
    /// when tagging offline queued actions.
    pub fn provider(&self) -> crate::db::Provider {
        match self {
            PrClient::GitHub(_) => crate::db::Provider::GitHub,
            PrClient::Forgejo(_) => crate::db::Provider::Forgejo,
        }
    }

    // ── Convenience helpers ───────────────────────────────────────────────────

    /// Return the label names currently on a PR.
    pub fn pr_label_names(&self, number: u64) -> Result<Vec<String>> {
        Ok(self
            .get_pr_labels(number)?
            .into_iter()
            .map(|l| l.name)
            .collect())
    }

    /// Return whether the PR is closed (state == "closed").
    pub fn pr_is_closed(&self, number: u64) -> Result<bool> {
        match self {
            PrClient::GitHub(gh) => {
                let issue = gh.get_issue(number)?;
                Ok(issue.is_closed())
            }
            PrClient::Forgejo(_) => {
                let pr = self.get_pr(number)?;
                Ok(pr.state == "closed")
            }
        }
    }
}
