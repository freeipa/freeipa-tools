use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::api::types::{CiStatus, Commit, IssueComment, Label, PrFile, PullRequest, ReviewComment};

pub struct GitHubClient {
    pub http: reqwest::blocking::Client,
    token: String,
    pub owner: String,
    pub repo: String,
}

/// GitHub issue fields (GitHub-specific: includes milestone, state, labels).
#[derive(Debug, Deserialize, Clone)]
pub struct GitHubIssue {
    pub state: String,
    pub labels: Vec<Label>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub milestone: Option<GitHubMilestone>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GitHubMilestone {
    pub title: String,
}

impl GitHubIssue {
    pub fn is_closed(&self) -> bool {
        self.state == "closed"
    }
}

/// Raw GitHub commit-status entry (one of potentially many per context).
#[derive(Debug, Deserialize, Clone)]
pub struct CommitStatus {
    pub state: String,
    pub context: String,
    #[serde(default)]
    pub target_url: Option<String>,
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

/// Percent-encode a string for use as a URL path segment (RFC 3986 unreserved chars pass through).
fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            // unreserved characters (RFC 3986 §2.3)
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            other => {
                out.push('%');
                out.push(char::from_digit((other >> 4) as u32, 16).unwrap().to_ascii_uppercase());
                out.push(char::from_digit((other & 0xf) as u32, 16).unwrap().to_ascii_uppercase());
            }
        }
    }
    out
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
        format!(
            "https://api.github.com/repos/{}/{}{}",
            self.owner, self.repo, path
        )
    }

    fn auth_header(&self) -> String {
        format!("token {}", self.token)
    }

    pub fn get_pr(&self, number: u64) -> Result<PullRequest> {
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
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("GitHub API error {} fetching PR {}: {}", status, number, body);
        }
        let pr: PullRequest = resp
            .json()
            .with_context(|| format!("Parsing PR {}", number))?;
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
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!(
                "GitHub API error {} fetching issue {}: {}",
                status,
                number,
                body
            );
        }
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

    pub fn list_prs(&self, state: &str) -> Result<Vec<PullRequest>> {
        self.list_prs_limited(state, usize::MAX, |_, _| {})
    }

    /// Fetch PRs stopping as soon as `limit` results are collected.
    /// Calls `on_page(page_number, collected_so_far)` after each page lands.
    pub fn list_prs_limited(
        &self,
        state: &str,
        limit: usize,
        mut on_page: impl FnMut(u32, usize),
    ) -> Result<Vec<PullRequest>> {
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
            let prs: Vec<PullRequest> = resp.json().with_context(|| "Parsing PR list")?;
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

    pub fn get_pr_commits(&self, number: u64) -> Result<Vec<Commit>> {
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
            let commits: Vec<Commit> = resp
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

    /// Get the most recent status per context, including the job result URL.
    pub fn most_recent_statuses(&self, sha: &str) -> Result<HashMap<String, CiStatus>> {
        let statuses = self.get_commit_statuses(sha)?;
        let mut result: HashMap<String, CiStatus> = HashMap::new();
        for s in statuses {
            result.entry(s.context).or_insert(CiStatus {
                state: s.state,
                url: s.target_url,
            });
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
        let url = self.api_url(&format!(
            "/issues/{}/labels/{}",
            number,
            encode_path_segment(label)
        ));
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

    pub fn create_pr(&self, title: &str, base: &str, head: &str, body: &str) -> Result<PullRequest> {
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
        let pr: PullRequest = resp.json().with_context(|| "Parsing created PR")?;
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
    pub fn get_last_issue_comments(&self, number: u64, n: usize) -> Result<Vec<IssueComment>> {
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
        let mut comments: Vec<IssueComment> =
            resp.json().with_context(|| "Parsing issue comments")?;
        comments.reverse(); // chronological order (oldest of the last-N first)
        Ok(comments)
    }

    /// Return all issue comments in chronological order, paging automatically.
    pub fn get_all_issue_comments(&self, number: u64) -> Result<Vec<IssueComment>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let url = self.api_url(&format!(
                "/issues/{}/comments?sort=created&direction=asc&per_page=100&page={}",
                number, page
            ));
            let resp = self
                .http
                .get(&url)
                .header("Authorization", self.auth_header())
                .header("Accept", "application/vnd.github.v3+json")
                .send()
                .with_context(|| format!("GET {}", url))?;
            let comments: Vec<IssueComment> = resp
                .json()
                .with_context(|| format!("Parsing comments for issue {}", number))?;
            if comments.is_empty() {
                break;
            }
            all.extend(comments);
            page += 1;
        }
        Ok(all)
    }

    /// Return the changed-file list for a PR (first page, up to 100 files).
    /// Each `PrFile` includes the `patch` field when available.
    pub fn get_pr_files(&self, number: u64) -> Result<Vec<PrFile>> {
        let url = self.api_url(&format!("/pulls/{}/files?per_page=100", number));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .with_context(|| format!("GET {}", url))?;
        let files: Vec<PrFile> = resp
            .json()
            .with_context(|| format!("Parsing files for PR {}", number))?;
        Ok(files)
    }

    /// Return all labels defined in the repository.
    pub fn list_repo_labels(&self) -> Result<Vec<Label>> {
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
            let labels: Vec<Label> = resp.json().with_context(|| "Parsing repo labels")?;
            if labels.is_empty() {
                break;
            }
            all.extend(labels);
            page += 1;
        }
        Ok(all)
    }

    /// Return all inline review comments for a PR.
    pub fn list_review_comments(&self, pr_number: u64) -> Result<Vec<ReviewComment>> {
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
            let comments: Vec<ReviewComment> =
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

