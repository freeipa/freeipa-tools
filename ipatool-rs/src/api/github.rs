use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::api::types::{CiStatus, Commit, IssueComment, Label, PrFile, PullRequest, ReviewComment, TicketOps};
use std::sync::{Arc, OnceLock};

pub struct GitHubClient {
    pub http: reqwest::blocking::Client,
    token: String,
    pub owner: String,
    pub repo: String,
}

/// GitHub issue fields (GitHub-specific: includes milestone, state, labels).
#[derive(Debug, Deserialize, Clone)]
pub struct GitHubIssue {
    #[serde(default)]
    pub number: u64,
    pub state: String,
    pub labels: Vec<Label>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub milestone: Option<GitHubMilestone>,
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
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

    pub fn list_issues_by_milestone(
        &self,
        state: &str,
        milestone_title: &str,
    ) -> Result<Vec<GitHubIssue>> {
        #[derive(Debug, Deserialize)]
        struct MilestoneInfo {
            number: u64,
            title: String,
        }

        const MAX_PAGES: u32 = 1_000;
        let mut milestone_number = None;
        let mut page = 1u32;
        let milestones_base = self.api_url("/milestones");
        'outer: loop {
            if page > MAX_PAGES {
                eprintln!(
                    "Warning: milestone pagination exceeded {} pages, results may be incomplete",
                    MAX_PAGES
                );
                break;
            }
            let url = reqwest::Url::parse_with_params(
                &milestones_base,
                &[
                    ("state", "all"),
                    ("per_page", "100"),
                    ("page", &page.to_string()),
                ],
            )
            .context("building GitHub milestones URL")?;
            let url = url.to_string();
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
                anyhow::bail!("GitHub list milestones failed ({}): {}", status, body);
            }
            let milestones: Vec<MilestoneInfo> = resp
                .json()
                .context("Parsing GitHub milestones")?;
            if milestones.is_empty() {
                break;
            }
            for m in &milestones {
                if m.title == milestone_title {
                    milestone_number = Some(m.number);
                    break 'outer;
                }
            }
            page += 1;
        }

        let milestone_num = match milestone_number {
            Some(n) => n,
            None => anyhow::bail!("Milestone '{}' not found on GitHub", milestone_title),
        };

        let issues_base = self.api_url("/issues");
        let milestone_str = milestone_num.to_string();
        let mut all_issues = Vec::new();
        page = 1;
        loop {
            if page > MAX_PAGES {
                eprintln!(
                    "Warning: issue pagination exceeded {} pages, results may be incomplete",
                    MAX_PAGES
                );
                break;
            }
            let url = reqwest::Url::parse_with_params(
                &issues_base,
                &[
                    ("state", state),
                    ("milestone", milestone_str.as_str()),
                    ("per_page", "100"),
                    ("page", &page.to_string()),
                ],
            )
            .context("building GitHub issues URL")?;
            let url = url.to_string();
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
                anyhow::bail!("GitHub list issues failed ({}): {}", status, body);
            }
            let issues: Vec<GitHubIssue> = resp
                .json()
                .context("Parsing GitHub issues")?;
            if issues.is_empty() {
                break;
            }
            let filtered: Vec<GitHubIssue> = issues
                .into_iter()
                .filter(|i| i.pull_request.is_none())
                .collect();
            all_issues.extend(filtered);
            page += 1;
        }
        Ok(all_issues)
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

// ── GitHub Issues as a ticket backend ────────────────────────────────────────

pub(crate) struct GitHubTicket {
    pub client: Arc<GitHubClient>,
    pub number: u64,
    pub comment_field_prefix: String,
    data: OnceLock<GitHubIssue>,
    comments: OnceLock<Vec<IssueComment>>,
}

impl GitHubTicket {
    pub fn new(client: Arc<GitHubClient>, number: u64, comment_field_prefix: String) -> Self {
        GitHubTicket {
            client,
            number,
            comment_field_prefix,
            data: OnceLock::new(),
            comments: OnceLock::new(),
        }
    }

    pub fn data(&self) -> Result<&GitHubIssue> {
        if let Some(d) = self.data.get() {
            return Ok(d);
        }
        println!("Retrieving GitHub issue #{}", self.number);
        let issue = self.client.get_issue(self.number)?;
        let _ = self.data.set(issue);
        Ok(self
            .data
            .get()
            .expect("OnceLock was just set above; this is a logic error if None"))
    }

    fn load_comments(&self) -> Result<&[IssueComment]> {
        if let Some(c) = self.comments.get() {
            return Ok(c);
        }
        let fetched = self.client.get_all_issue_comments(self.number)?;
        let _ = self.comments.set(fetched);
        Ok(self
            .comments
            .get()
            .expect("OnceLock was just set above; this is a logic error if None"))
    }

    /// Scan the issue body and all comments for a line of the form
    /// `<prefix><name>: <value>` and return the joined values.
    fn comment_field(&self, name: &str) -> Result<Option<String>> {
        let prefix = &self.comment_field_prefix;
        let needle = format!("{}:", name);
        let mut values: Vec<String> = Vec::new();

        let mut scan = |text: &str| {
            for line in text.lines() {
                let rest = if prefix.is_empty() {
                    line
                } else {
                    match line.strip_prefix(prefix.as_str()) {
                        Some(r) => r.trim_start(),
                        None => continue,
                    }
                };
                if let Some(val) = rest.strip_prefix(&*needle) {
                    let v = val.trim();
                    if !v.is_empty() {
                        values.push(v.to_string());
                    }
                }
            }
        };

        let issue = self.data()?;
        if let Some(ref body) = issue.body {
            scan(body);
        }
        for comment in self.load_comments()? {
            scan(&comment.body);
        }

        if values.is_empty() {
            Ok(None)
        } else {
            Ok(Some(values.join(" ")))
        }
    }

    pub fn reviewer(&self) -> Result<Option<String>> { self.comment_field("reviewer") }
    pub fn rhbz(&self) -> Result<Option<String>> { self.comment_field("rhbz") }
    pub fn title(&self) -> Result<String> { Ok(self.data()?.title.clone()) }
    pub fn is_closed(&self) -> Result<bool> { Ok(self.data()?.is_closed()) }
    pub fn comment(&self, text: &str) -> Result<()> { self.client.create_comment(self.number, text) }
    pub fn close(&self) -> Result<()> { self.client.close_issue(self.number) }
    pub fn milestone(&self) -> Result<Option<String>> {
        Ok(self.data()?.milestone.as_ref().map(|m| m.title.clone()))
    }
}

impl TicketOps for GitHubTicket {
    fn number(&self) -> u64 { self.number }
    fn reviewer(&self) -> Result<Option<String>> { self.reviewer() }
    fn rhbz(&self) -> Result<Option<String>> { self.rhbz() }
    fn milestone(&self) -> Result<Option<String>> { self.milestone() }
    fn title(&self) -> Result<String> { self.title() }
    fn is_closed(&self) -> Result<bool> { self.is_closed() }
    fn comment(&self, text: &str) -> Result<()> { self.comment(text) }
    fn close(&self) -> Result<()> { self.close() }
}

