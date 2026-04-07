use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub struct GitHubClient {
    pub http: reqwest::blocking::Client,
    token: String,
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GitHubLabel {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GitHubUser {
    pub login: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GitHubRef {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GitHubPR {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub html_url: String,
    pub head: GitHubRef,
    pub base: GitHubRef,
    pub user: GitHubUser,
    pub mergeable: Option<bool>,
    pub merged: Option<bool>,
    #[serde(default)]
    pub labels: Vec<GitHubLabel>,
    #[serde(default)]
    pub body: Option<String>,
    // Counters present in the list API response
    #[serde(default)]
    pub commits: Option<u64>,
    #[serde(default)]
    pub comments: Option<u64>,
    #[serde(default)]
    pub review_comments: Option<u64>,
    // Only in single-PR detail response, not in the list response
    #[serde(default)]
    pub additions: Option<u64>,
    #[serde(default)]
    pub deletions: Option<u64>,
    #[serde(default)]
    pub changed_files: Option<u64>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GitHubComment {
    pub id: u64,
    pub user: GitHubUser,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GitHubFile {
    pub filename: String,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub changes: u64,
    #[serde(default)]
    pub previous_filename: Option<String>,
    /// Unified diff patch for this file (absent for binary / very large files).
    #[serde(default)]
    pub patch: Option<String>,
}

/// A single pull-request review comment (line-level).
#[derive(Debug, Deserialize, Clone)]
pub struct GitHubReviewComment {
    pub id: u64,
    pub user: GitHubUser,
    pub body: String,
    pub path: String,
    /// Line number in the new file (RIGHT side). Absent on some legacy comments.
    pub line: Option<u64>,
    /// Line number in the original file (LEFT side).
    pub original_line: Option<u64>,
    /// "LEFT" or "RIGHT".
    pub side: Option<String>,
    pub diff_hunk: String,
    pub created_at: String,
    pub commit_id: String,
    #[serde(default)]
    pub in_reply_to_id: Option<u64>,
}

impl GitHubPR {
    pub fn is_merged(&self) -> bool {
        self.merged.unwrap_or(false)
    }

    pub fn is_open(&self) -> bool {
        self.state == "open"
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct GitHubIssue {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub labels: Vec<GitHubLabel>,
}

impl GitHubIssue {
    pub fn label_names(&self) -> Vec<String> {
        self.labels.iter().map(|l| l.name.clone()).collect()
    }

    pub fn is_closed(&self) -> bool {
        self.state == "closed"
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ParentRef {
    pub sha: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitDetails {
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GitHubCommit {
    pub sha: String,
    pub commit: CommitDetails,
    pub parents: Vec<ParentRef>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CommitStatus {
    pub state: String,
    pub context: String,
}

#[derive(Serialize)]
struct AddLabelsBody {
    labels: Vec<String>,
}

#[derive(Serialize)]
struct CommentBody {
    body: String,
}

#[derive(Serialize)]
struct CloseIssueBody {
    state: String,
}

#[derive(Serialize)]
struct CreateReviewCommentBody {
    body: String,
    commit_id: String,
    path: String,
    line: u64,
    side: String,
}

#[derive(Serialize)]
struct CreatePRBody {
    title: String,
    base: String,
    head: String,
    body: String,
}

impl GitHubClient {
    pub fn new(token: &str, owner: &str, repo: &str) -> Result<Self> {
        use std::time::Duration;
        let http = reqwest::blocking::Client::builder()
            .user_agent("ipatool/1.0")
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build GitHub HTTP client")?;
        Ok(GitHubClient {
            http,
            token: token.to_string(),
            owner: owner.to_string(),
            repo: repo.to_string(),
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://api.github.com/repos/{}/{}{}", self.owner, self.repo, path)
    }

    fn auth_header(&self) -> String {
        format!("token {}", self.token)
    }

    pub fn get_pr(&self, number: u64) -> Result<GitHubPR> {
        let url = self.api_url(&format!("/pulls/{}", number));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .with_context(|| format!("GET {}", url))?;
        if resp.status().as_u16() == 404 {
            anyhow::bail!("Pull request {} not found", number);
        }
        let pr: GitHubPR = resp.json().with_context(|| format!("Parsing PR {}", number))?;
        Ok(pr)
    }

    pub fn get_issue(&self, number: u64) -> Result<GitHubIssue> {
        let url = self.api_url(&format!("/issues/{}", number));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .with_context(|| format!("GET {}", url))?;
        let issue: GitHubIssue = resp
            .json()
            .with_context(|| format!("Parsing issue {}", number))?;
        Ok(issue)
    }

    pub fn is_pr_merged(&self, number: u64) -> Result<bool> {
        let url = self.api_url(&format!("/pulls/{}/merge", number));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .with_context(|| format!("GET {}", url))?;
        // 204 = merged, 404 = not merged
        Ok(resp.status().as_u16() == 204)
    }

    pub fn list_prs(&self, state: &str) -> Result<Vec<GitHubPR>> {
        self.list_prs_limited(state, usize::MAX, |_, _| {})
    }

    /// Fetch PRs stopping as soon as `limit` results are collected.
    /// Calls `on_page(page_number, collected_so_far)` after each page lands.
    pub fn list_prs_limited(
        &self,
        state: &str,
        limit: usize,
        mut on_page: impl FnMut(u32, usize),
    ) -> Result<Vec<GitHubPR>> {
        let mut all_prs = Vec::new();
        let mut page = 1u32;
        loop {
            let url = self.api_url(&format!(
                "/pulls?state={}&per_page=100&page={}",
                state, page
            ));
            let resp = self
                .http
                .get(&url)
                .header("Authorization", self.auth_header())
                .header("Accept", "application/vnd.github.v3+json")
                .send()
                .with_context(|| format!("GET {}", url))?;
            let prs: Vec<GitHubPR> = resp.json().with_context(|| "Parsing PR list")?;
            if prs.is_empty() {
                break;
            }
            all_prs.extend(prs);
            on_page(page, all_prs.len());
            if all_prs.len() >= limit {
                break;
            }
            page += 1;
        }
        all_prs.truncate(limit);
        Ok(all_prs)
    }

    pub fn get_pr_commits(&self, number: u64) -> Result<Vec<GitHubCommit>> {
        let mut all_commits = Vec::new();
        let mut page = 1u32;
        loop {
            let url = self.api_url(&format!(
                "/pulls/{}/commits?per_page=100&page={}",
                number, page
            ));
            let resp = self
                .http
                .get(&url)
                .header("Authorization", self.auth_header())
                .header("Accept", "application/vnd.github.v3+json")
                .send()
                .with_context(|| format!("GET {}", url))?;
            let commits: Vec<GitHubCommit> = resp
                .json()
                .with_context(|| format!("Parsing commits for PR {}", number))?;
            if commits.is_empty() {
                break;
            }
            all_commits.extend(commits);
            page += 1;
        }
        Ok(all_commits)
    }

    pub fn get_commit_statuses(&self, sha: &str) -> Result<Vec<CommitStatus>> {
        let url = self.api_url(&format!("/commits/{}/statuses", sha));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .with_context(|| format!("GET {}", url))?;
        let statuses: Vec<CommitStatus> = resp
            .json()
            .with_context(|| format!("Parsing statuses for {}", sha))?;
        Ok(statuses)
    }

    /// Get the most recent status per context
    pub fn most_recent_statuses(&self, sha: &str) -> Result<HashMap<String, String>> {
        let statuses = self.get_commit_statuses(sha)?;
        let mut result = HashMap::new();
        for s in statuses {
            result.entry(s.context).or_insert(s.state);
        }
        Ok(result)
    }

    pub fn get_commit_patch(&self, sha: &str) -> Result<Vec<u8>> {
        let url = self.api_url(&format!("/commits/{}", sha));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3.patch")
            .send()
            .with_context(|| format!("GET patch for {}", sha))?;
        Ok(resp.bytes()?.to_vec())
    }

    pub fn add_labels(&self, number: u64, labels: &[&str]) -> Result<()> {
        let url = self.api_url(&format!("/issues/{}/labels", number));
        let body = AddLabelsBody {
            labels: labels.iter().map(|s| s.to_string()).collect(),
        };
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .json(&body)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("add_labels failed ({}): {}", status, body);
        }
        Ok(())
    }

    pub fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        let url = self.api_url(&format!("/issues/{}/labels/{}", number, label));
        let resp = self
            .http
            .delete(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .with_context(|| format!("DELETE {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("remove_label failed ({}): {}", status, body);
        }
        Ok(())
    }

    pub fn create_comment(&self, number: u64, text: &str) -> Result<()> {
        let url = self.api_url(&format!("/issues/{}/comments", number));
        let body = CommentBody {
            body: text.to_string(),
        };
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .json(&body)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("create_comment failed ({}): {}", status, body);
        }
        Ok(())
    }

    pub fn close_issue(&self, number: u64) -> Result<()> {
        let url = self.api_url(&format!("/issues/{}", number));
        let body = CloseIssueBody {
            state: "closed".to_string(),
        };
        let resp = self
            .http
            .patch(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .json(&body)
            .send()
            .with_context(|| format!("PATCH {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("close_issue failed ({}): {}", status, body);
        }
        Ok(())
    }

    pub fn create_pr(
        &self,
        title: &str,
        base: &str,
        head: &str,
        body: &str,
    ) -> Result<GitHubPR> {
        let url = self.api_url("/pulls");
        let req_body = CreatePRBody {
            title: title.to_string(),
            base: base.to_string(),
            head: head.to_string(),
            body: body.to_string(),
        };
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .json(&req_body)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("create_pr failed ({}): {}", status, body);
        }
        let pr: GitHubPR = resp.json().with_context(|| "Parsing created PR")?;
        Ok(pr)
    }

    pub fn get_authenticated_user_login(&self) -> Result<String> {
        let url = "https://api.github.com/user";
        let resp = self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .with_context(|| "GET /user")?;
        let user: serde_json::Value = resp.json().with_context(|| "Parsing /user")?;
        user["login"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("GitHub /user response missing 'login' field"))
    }

    /// Return the last `n` issue comments in chronological order (newest-last).
    /// Uses `direction=desc` so only one page is needed regardless of total count.
    pub fn get_last_issue_comments(&self, number: u64, n: usize) -> Result<Vec<GitHubComment>> {
        let url = self.api_url(&format!(
            "/issues/{}/comments?sort=created&direction=desc&per_page={}",
            number, n
        ));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .with_context(|| format!("GET {}", url))?;
        let mut comments: Vec<GitHubComment> =
            resp.json().with_context(|| "Parsing issue comments")?;
        comments.reverse(); // chronological order (oldest of the last-N first)
        Ok(comments)
    }

    /// Return the changed-file list for a PR (first page, up to 100 files).
    /// Each `GitHubFile` includes the `patch` field when available.
    pub fn get_pr_files(&self, number: u64) -> Result<Vec<GitHubFile>> {
        let url = self.api_url(&format!("/pulls/{}/files?per_page=100", number));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .with_context(|| format!("GET {}", url))?;
        let files: Vec<GitHubFile> =
            resp.json().with_context(|| format!("Parsing files for PR {}", number))?;
        Ok(files)
    }

    /// Return all labels defined in the repository.
    pub fn list_repo_labels(&self) -> Result<Vec<GitHubLabel>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let url = self.api_url(&format!("/labels?per_page=100&page={}", page));
            let resp = self
                .http
                .get(&url)
                .header("Authorization", self.auth_header())
                .header("Accept", "application/vnd.github.v3+json")
                .send()
                .with_context(|| format!("GET {}", url))?;
            let labels: Vec<GitHubLabel> = resp.json().with_context(|| "Parsing repo labels")?;
            if labels.is_empty() {
                break;
            }
            all.extend(labels);
            page += 1;
        }
        Ok(all)
    }

    /// Return all inline review comments for a PR.
    pub fn list_review_comments(&self, pr_number: u64) -> Result<Vec<GitHubReviewComment>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let url = self.api_url(&format!(
                "/pulls/{}/comments?per_page=100&page={}",
                pr_number, page
            ));
            let resp = self
                .http
                .get(&url)
                .header("Authorization", self.auth_header())
                .header("Accept", "application/vnd.github.v3+json")
                .send()
                .with_context(|| format!("GET {}", url))?;
            let comments: Vec<GitHubReviewComment> =
                resp.json().with_context(|| "Parsing review comments")?;
            if comments.is_empty() {
                break;
            }
            all.extend(comments);
            page += 1;
        }
        Ok(all)
    }

    /// Post a single inline review comment on a specific line of a PR.
    pub fn create_review_comment(
        &self,
        pr_number: u64,
        commit_id: &str,
        path: &str,
        line: u64,
        body: &str,
    ) -> Result<()> {
        let url = self.api_url(&format!("/pulls/{}/comments", pr_number));
        let req_body = CreateReviewCommentBody {
            body: body.to_string(),
            commit_id: commit_id.to_string(),
            path: path.to_string(),
            line,
            side: "RIGHT".to_string(),
        };
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .json(&req_body)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("create_review_comment failed ({}): {}", status, body);
        }
        Ok(())
    }
}

/// Sort commits topologically (linear chain, no merges)
pub fn sorted_commits(commits: Vec<GitHubCommit>) -> Result<Vec<GitHubCommit>> {
    for c in &commits {
        if c.parents.len() != 1 {
            anyhow::bail!(
                "Commit {} has {} parents (merge commit not supported)",
                c.sha,
                c.parents.len()
            );
        }
    }

    use std::collections::HashSet;
    let commit_ids: HashSet<&str> = commits.iter().map(|c| c.sha.as_str()).collect();

    // Find root commit (parent not in set)
    let first = commits
        .iter()
        .find(|c| !commit_ids.contains(c.parents[0].sha.as_str()))
        .ok_or_else(|| anyhow::anyhow!("No first commit found in chain"))?;

    let mut result = vec![first.clone()];
    let mut parent_id = first.sha.clone();

    while result.len() < commits.len() {
        let next = commits
            .iter()
            .find(|c| c.parents[0].sha == parent_id)
            .ok_or_else(|| {
                anyhow::anyhow!("Commit {} should have child but none found", parent_id)
            })?;
        parent_id = next.sha.clone();
        result.push(next.clone());
    }

    Ok(result)
}

/// Format labels with terminal 24-bit color codes
pub fn labels_colorize(labels: &[GitHubLabel], color_enabled: bool) -> String {
    let parts: Vec<String> = labels
        .iter()
        .map(|l| {
            if color_enabled && l.color.len() >= 6 {
                if let Ok(r) = u8::from_str_radix(&l.color[0..2], 16) {
                    if let Ok(g) = u8::from_str_radix(&l.color[2..4], 16) {
                        if let Ok(b) = u8::from_str_radix(&l.color[4..6], 16) {
                            return format!("\x1b[38;2;{};{};{}m{}\x1b[0m", r, g, b, l.name);
                        }
                    }
                }
            }
            l.name.clone()
        })
        .collect();
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_commit(sha: &str, parent_sha: &str, message: &str) -> GitHubCommit {
        GitHubCommit {
            sha: sha.to_string(),
            commit: CommitDetails { message: message.to_string() },
            parents: vec![ParentRef { sha: parent_sha.to_string() }],
        }
    }

    // ── GitHubPR deserialization ───────────────────────────────────────────────

    #[test]
    fn test_pr_deserialization_minimal() {
        let json = r#"{
            "number": 42,
            "title": "Fix something",
            "state": "open",
            "html_url": "https://github.com/test/repo/pull/42",
            "head": {"ref": "feature", "sha": "abc123"},
            "base": {"ref": "master", "sha": "def456"},
            "user": {"login": "testuser"}
        }"#;
        let pr: GitHubPR = serde_json::from_str(json).unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.title, "Fix something");
        assert_eq!(pr.state, "open");
        assert!(pr.labels.is_empty());
        assert!(pr.merged.is_none());
        assert!(pr.updated_at.is_none());
        assert!(pr.body.is_none());
    }

    #[test]
    fn test_pr_deserialization_full() {
        let json = r#"{
            "number": 99,
            "title": "Full PR",
            "state": "closed",
            "html_url": "https://github.com/org/repo/pull/99",
            "head": {"ref": "my-feature", "sha": "deadbeef"},
            "base": {"ref": "master", "sha": "cafebabe"},
            "user": {"login": "contributor"},
            "mergeable": true,
            "merged": true,
            "labels": [{"name": "ack", "color": "00ff00"}],
            "body": "PR description",
            "commits": 3,
            "additions": 50,
            "deletions": 10,
            "changed_files": 4,
            "updated_at": "2024-06-01T12:00:00Z"
        }"#;
        let pr: GitHubPR = serde_json::from_str(json).unwrap();
        assert_eq!(pr.number, 99);
        assert!(pr.is_merged());
        assert!(!pr.is_open());
        assert_eq!(pr.labels.len(), 1);
        assert_eq!(pr.labels[0].name, "ack");
        assert_eq!(pr.updated_at.as_deref(), Some("2024-06-01T12:00:00Z"));
        assert_eq!(pr.additions, Some(50));
    }

    #[test]
    fn test_pr_is_open_true() {
        let json = r#"{"number":1,"title":"t","state":"open","html_url":"u","head":{"ref":"r","sha":"s"},"base":{"ref":"m","sha":"b"},"user":{"login":"u"}}"#;
        let pr: GitHubPR = serde_json::from_str(json).unwrap();
        assert!(pr.is_open());
        assert!(!pr.is_merged());
    }

    #[test]
    fn test_pr_is_open_false_when_closed() {
        let json = r#"{"number":1,"title":"t","state":"closed","html_url":"u","head":{"ref":"r","sha":"s"},"base":{"ref":"m","sha":"b"},"user":{"login":"u"}}"#;
        let pr: GitHubPR = serde_json::from_str(json).unwrap();
        assert!(!pr.is_open());
    }

    #[test]
    fn test_pr_is_merged_absent_defaults_false() {
        // No "merged" field → is_merged() should be false
        let json = r#"{"number":1,"title":"t","state":"open","html_url":"u","head":{"ref":"r","sha":"s"},"base":{"ref":"m","sha":"b"},"user":{"login":"u"}}"#;
        let pr: GitHubPR = serde_json::from_str(json).unwrap();
        assert!(!pr.is_merged());
    }

    #[test]
    fn test_pr_serialization_roundtrip() {
        let json = r#"{"number":42,"title":"Test","state":"open","html_url":"https://github.com/t/r/pull/42","head":{"ref":"feature","sha":"abc"},"base":{"ref":"master","sha":"def"},"user":{"login":"u"},"mergeable":null,"merged":null,"labels":[],"body":null,"commits":null,"comments":null,"review_comments":null,"additions":null,"deletions":null,"changed_files":null,"updated_at":null}"#;
        let pr: GitHubPR = serde_json::from_str(json).unwrap();
        let re_json = serde_json::to_string(&pr).unwrap();
        let pr2: GitHubPR = serde_json::from_str(&re_json).unwrap();
        assert_eq!(pr2.number, 42);
        assert_eq!(pr2.title, "Test");
    }

    // ── GitHubFile deserialization ─────────────────────────────────────────────

    #[test]
    fn test_file_deserialization_with_rename() {
        let json = r#"{
            "filename": "src/new_name.rs",
            "status": "renamed",
            "additions": 0,
            "deletions": 0,
            "changes": 0,
            "previous_filename": "src/old_name.rs",
            "patch": "@@ -1 +1 @@\n-old\n+new"
        }"#;
        let file: GitHubFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.previous_filename.as_deref(), Some("src/old_name.rs"));
        assert_eq!(file.status, "renamed");
        assert!(file.patch.is_some());
    }

    #[test]
    fn test_file_deserialization_no_patch() {
        let json = r#"{
            "filename": "binary.bin",
            "status": "added",
            "additions": 0,
            "deletions": 0,
            "changes": 0
        }"#;
        let file: GitHubFile = serde_json::from_str(json).unwrap();
        assert!(file.patch.is_none());
        assert!(file.previous_filename.is_none());
    }

    // ── GitHubComment deserialization ──────────────────────────────────────────

    #[test]
    fn test_comment_deserialization() {
        let json = r#"{
            "id": 123,
            "user": {"login": "reviewer"},
            "body": "LGTM!",
            "created_at": "2024-01-15T09:00:00Z"
        }"#;
        let comment: GitHubComment = serde_json::from_str(json).unwrap();
        assert_eq!(comment.id, 123);
        assert_eq!(comment.user.login, "reviewer");
        assert_eq!(comment.body, "LGTM!");
    }

    #[test]
    fn test_comment_serialization_roundtrip() {
        let comment = GitHubComment {
            id: 1,
            user: GitHubUser { login: "tester".to_string() },
            body: "hello".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&comment).unwrap();
        let c2: GitHubComment = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.body, "hello");
    }

    // ── sorted_commits ─────────────────────────────────────────────────────────

    #[test]
    fn test_sorted_commits_single() {
        let c = make_commit("sha1", "outside_root", "Only commit");
        let sorted = sorted_commits(vec![c]).unwrap();
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].sha, "sha1");
    }

    #[test]
    fn test_sorted_commits_already_ordered() {
        let c1 = make_commit("sha1", "base", "First");
        let c2 = make_commit("sha2", "sha1", "Second");
        let sorted = sorted_commits(vec![c1, c2]).unwrap();
        assert_eq!(sorted[0].sha, "sha1");
        assert_eq!(sorted[1].sha, "sha2");
    }

    #[test]
    fn test_sorted_commits_reversed_input() {
        let c1 = make_commit("sha1", "base", "First");
        let c2 = make_commit("sha2", "sha1", "Second");
        // Feed them in reverse order
        let sorted = sorted_commits(vec![c2, c1]).unwrap();
        assert_eq!(sorted[0].sha, "sha1");
        assert_eq!(sorted[1].sha, "sha2");
    }

    #[test]
    fn test_sorted_commits_three() {
        let c1 = make_commit("a", "root", "First");
        let c2 = make_commit("b", "a", "Second");
        let c3 = make_commit("c", "b", "Third");
        // Shuffle input
        let sorted = sorted_commits(vec![c3, c1, c2]).unwrap();
        assert_eq!(sorted[0].sha, "a");
        assert_eq!(sorted[1].sha, "b");
        assert_eq!(sorted[2].sha, "c");
    }

    #[test]
    fn test_sorted_commits_merge_commit_fails() {
        let merge = GitHubCommit {
            sha: "merge_sha".to_string(),
            commit: CommitDetails { message: "Merge branch".to_string() },
            parents: vec![
                ParentRef { sha: "p1".to_string() },
                ParentRef { sha: "p2".to_string() },
            ],
        };
        let result = sorted_commits(vec![merge]);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("2 parents") || msg.contains("parents"));
    }

    // ── labels_colorize ────────────────────────────────────────────────────────

    #[test]
    fn test_labels_colorize_no_color() {
        let labels = vec![
            GitHubLabel { name: "ack".to_string(), color: "00ff00".to_string() },
            GitHubLabel { name: "pushed".to_string(), color: "ff0000".to_string() },
        ];
        let result = labels_colorize(&labels, false);
        assert_eq!(result, "ack,pushed");
    }

    #[test]
    fn test_labels_colorize_with_color() {
        let labels = vec![
            GitHubLabel { name: "ack".to_string(), color: "00ff00".to_string() },
        ];
        let result = labels_colorize(&labels, true);
        assert!(result.contains("\x1b[38;2;"));
        assert!(result.contains("ack"));
        assert!(result.contains("\x1b[0m"));
    }

    #[test]
    fn test_labels_colorize_empty() {
        let result = labels_colorize(&[], false);
        assert!(result.is_empty());
    }

    #[test]
    fn test_labels_colorize_single_no_comma() {
        let labels = vec![GitHubLabel { name: "ack".to_string(), color: "00ff00".to_string() }];
        let result = labels_colorize(&labels, false);
        assert!(!result.contains(','));
        assert_eq!(result, "ack");
    }
}
