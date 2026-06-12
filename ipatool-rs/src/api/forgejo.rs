use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::api::types::{CiStatus, Commit, IssueComment, Label, PrFile, PullRequest, ReviewComment, TicketOps};

pub struct ForgejoClient {
    pub http: reqwest::blocking::Client,
    token: String,
    pub base_url: String,
    pub owner: String,
    pub repo: String,
}

/// Forgejo label — includes the numeric `id` needed for label deletion.
#[derive(Debug, Deserialize, Clone)]
pub struct ForgejoLabelId {
    pub id: u64,
    pub name: String,
    pub color: String,
}

/// Internal struct for adding labels (Forgejo requires IDs, not names).
#[derive(Serialize)]
struct AddLabelsByIdBody {
    labels: Vec<u64>,
}

/// Internal struct for closing a PR via PATCH.
#[derive(Serialize)]
struct PatchPrBody<'a> {
    state: &'a str,
}

/// Internal struct for creating a PR.
#[derive(Serialize)]
struct CreateForgejoPrBody {
    title: String,
    head: String,
    base: String,
    body: String,
}

/// Internal struct for creating a label.
#[derive(Serialize)]
struct CreateLabelBody<'a> {
    name: &'a str,
    color: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct ForgejoMilestone {
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct ForgejoIssue {
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    pub milestone: Option<ForgejoMilestone>,
}

impl ForgejoIssue {
    pub fn is_closed(&self) -> bool {
        self.state == "closed"
    }

    pub fn milestone_title(&self) -> Option<&str> {
        self.milestone.as_ref().map(|m| m.title.as_str())
    }
}

#[derive(Serialize)]
struct CommentBody<'a> {
    body: &'a str,
}

#[derive(Serialize)]
struct EditIssueBody<'a> {
    state: &'a str,
}

impl ForgejoClient {
    pub fn new(base_url: &str, token: &str, owner: &str, repo: &str) -> Result<Self> {
        use std::time::Duration;
        let http = reqwest::blocking::Client::builder()
            .user_agent("ipatool/1.0")
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build Forgejo HTTP client")?;
        // Normalize base_url: remove trailing slash
        let base_url = base_url.trim_end_matches('/').to_string();
        Ok(ForgejoClient {
            http,
            token: token.to_string(),
            base_url,
            owner: owner.to_string(),
            repo: repo.to_string(),
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}/api/v1{}", self.base_url, path)
    }

    pub fn get_issue(&self, number: u64) -> Result<ForgejoIssue> {
        let url = self.api_url(&format!(
            "/repos/{}/{}/issues/{}",
            self.owner, self.repo, number
        ));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .with_context(|| format!("GET {}", url))?;
        if resp.status().as_u16() == 404 {
            anyhow::bail!("Issue {} not found", number);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo get_issue failed ({}): {}", status, body);
        }
        let issue: ForgejoIssue = resp
            .json()
            .with_context(|| format!("Parsing forgejo issue {}", number))?;
        Ok(issue)
    }

    pub fn comment_issue(&self, number: u64, text: &str) -> Result<()> {
        let url = self.api_url(&format!(
            "/repos/{}/{}/issues/{}/comments",
            self.owner, self.repo, number
        ));
        let body = CommentBody { body: text };
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(&body)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo comment failed ({}): {}", status, body);
        }
        Ok(())
    }

    pub fn close_issue(&self, number: u64) -> Result<()> {
        let url = self.api_url(&format!(
            "/repos/{}/{}/issues/{}",
            self.owner, self.repo, number
        ));
        let body = EditIssueBody { state: "closed" };
        let resp = self
            .http
            .patch(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(&body)
            .send()
            .with_context(|| format!("PATCH {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo close issue failed ({}): {}", status, body);
        }
        Ok(())
    }

    // ── Pull-request methods ───────────────────────────────────────────────────

    /// List pull requests.  `state` is one of "open", "closed", "all".
    /// Returns PRs as `GitHubPR` (Forgejo's JSON shape is compatible).
    pub fn list_prs(&self, state: &str) -> Result<Vec<PullRequest>> {
        self.list_prs_limited(state, usize::MAX, |_, _| {})
    }

    pub fn list_prs_limited(
        &self,
        state: &str,
        limit: usize,
        mut on_page: impl FnMut(u32, usize),
    ) -> Result<Vec<PullRequest>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        let forgejo_state = match state {
            "all" => "all",
            "closed" => "closed",
            _ => "open",
        };
        loop {
            let url = self.api_url(&format!(
                "/repos/{}/{}/pulls?state={}&page={}&limit=50&type=pulls",
                self.owner, self.repo, forgejo_state, page
            ));
            let resp = self
                .http
                .get(&url)
                .header("Authorization", format!("token {}", self.token))
                .send()
                .with_context(|| format!("GET {}", url))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().unwrap_or_default();
                anyhow::bail!("Forgejo list_prs failed ({}): {}", status, body);
            }
            let prs: Vec<PullRequest> =
                resp.json().with_context(|| "Parsing Forgejo PR list")?;
            if prs.is_empty() {
                break;
            }
            all.extend(prs);
            on_page(page, all.len());
            if all.len() >= limit {
                break;
            }
            page += 1;
        }
        all.truncate(limit);
        Ok(all)
    }

    pub fn get_pr(&self, number: u64) -> Result<PullRequest> {
        let url = self.api_url(&format!(
            "/repos/{}/{}/pulls/{}",
            self.owner, self.repo, number
        ));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .with_context(|| format!("GET {}", url))?;
        if resp.status().as_u16() == 404 {
            anyhow::bail!("Pull request {} not found", number);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo get_pr failed ({}): {}", status, body);
        }
        let pr: PullRequest = resp
            .json()
            .with_context(|| format!("Parsing Forgejo PR {}", number))?;
        Ok(pr)
    }

    pub fn is_pr_merged(&self, number: u64) -> Result<bool> {
        let pr = self.get_pr(number)?;
        Ok(pr.is_merged())
    }

    pub fn get_pr_commits(&self, number: u64) -> Result<Vec<Commit>> {
        let url = self.api_url(&format!(
            "/repos/{}/{}/pulls/{}/commits?limit=50",
            self.owner, self.repo, number
        ));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .with_context(|| format!("GET {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo get_pr_commits failed ({}): {}", status, body);
        }
        let commits: Vec<Commit> = resp
            .json()
            .with_context(|| format!("Parsing commits for PR {}", number))?;
        if commits.len() >= 50 {
            anyhow::bail!(
                "PR {} returned 50 or more commits; pagination is not yet implemented. \
                 Fetched only 50 commits (hard limit reached). Aborting to avoid incomplete patch set.",
                number
            );
        }
        Ok(commits)
    }

    /// Fetch a single commit as a unified-diff patch.
    /// Uses the Forgejo web endpoint `{base_url}/{owner}/{repo}/commit/{sha}.patch`
    /// which returns the `git format-patch` output without requiring API auth.
    pub fn get_commit_patch(&self, sha: &str) -> Result<Vec<u8>> {
        let url = format!(
            "{}/{}/{}/commit/{}.patch",
            self.base_url, self.owner, self.repo, sha
        );
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .with_context(|| format!("GET {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo get_commit_patch failed ({}): {}", status, body);
        }
        Ok(resp.bytes()?.to_vec())
    }

    /// CI status check: maps Forgejo statuses to a `CiStatus` for the caller.
    pub fn most_recent_statuses(
        &self,
        sha: &str,
    ) -> Result<HashMap<String, CiStatus>> {
        #[derive(Deserialize)]
        struct ForgejoStatus {
            state: String,
            context: String,
            #[serde(default)]
            target_url: Option<String>,
        }
        let url = self.api_url(&format!(
            "/repos/{}/{}/statuses/{}?page=1&limit=50",
            self.owner, self.repo, sha
        ));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .with_context(|| format!("GET {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo most_recent_statuses failed ({}): {}", status, body);
        }
        let statuses: Vec<ForgejoStatus> =
            resp.json().with_context(|| "Parsing Forgejo statuses")?;
        if statuses.len() == 50 {
            eprintln!(
                "Warning: CI status results for commit {} may be truncated at 50 entries.",
                sha
            );
        }
        let mut result = HashMap::new();
        for s in statuses {
            result
                .entry(s.context)
                .or_insert(CiStatus {
                    state: s.state,
                    url: s.target_url,
                });
        }
        Ok(result)
    }

    pub fn get_pr_files(&self, number: u64) -> Result<Vec<PrFile>> {
        let url = self.api_url(&format!(
            "/repos/{}/{}/pulls/{}/files?limit=100",
            self.owner, self.repo, number
        ));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .with_context(|| format!("GET {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo get_pr_files failed ({}): {}", status, body);
        }
        let files: Vec<PrFile> =
            resp.json().with_context(|| "Parsing Forgejo PR files")?;
        if files.len() == 100 {
            eprintln!(
                "Warning: get_pr_files returned exactly 100 files for PR {}; \
                 results may be truncated (hard limit reached)",
                number
            );
        }
        Ok(files)
    }

    /// Return all labels defined in this repository, with their numeric IDs.
    pub fn list_repo_labels_with_id(&self) -> Result<Vec<ForgejoLabelId>> {
        const MAX_PAGES: u32 = 1_000;
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let url = self.api_url(&format!(
                "/repos/{}/{}/labels?page={}&limit=50",
                self.owner, self.repo, page
            ));
            let resp = self
                .http
                .get(&url)
                .header("Authorization", format!("token {}", self.token))
                .send()
                .with_context(|| format!("GET {}", url))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().unwrap_or_default();
                anyhow::bail!(
                    "Forgejo list_repo_labels_with_id failed ({}): {}",
                    status,
                    body
                );
            }
            let labels: Vec<ForgejoLabelId> =
                resp.json().with_context(|| "Parsing Forgejo repo labels")?;
            if labels.is_empty() {
                break;
            }
            all.extend(labels);
            page += 1;
            if page > MAX_PAGES {
                eprintln!(
                    "Warning: pagination in list_repo_labels_with_id exceeded {} pages; results may be incomplete.",
                    MAX_PAGES
                );
                break;
            }
        }
        Ok(all)
    }

    /// Return all labels without IDs.
    pub fn list_repo_labels(&self) -> Result<Vec<Label>> {
        let labels = self.list_repo_labels_with_id()?;
        Ok(labels
            .into_iter()
            .map(|l| Label {
                name: l.name,
                color: l.color,
            })
            .collect())
    }

    /// Return the labels currently applied to issue/PR `number`, with IDs.
    pub fn get_issue_labels_with_id(&self, number: u64) -> Result<Vec<ForgejoLabelId>> {
        let url = self.api_url(&format!(
            "/repos/{}/{}/issues/{}/labels",
            self.owner, self.repo, number
        ));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .with_context(|| format!("GET {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!(
                "Forgejo get_issue_labels_with_id failed ({}): {}",
                status,
                body
            );
        }
        let labels: Vec<ForgejoLabelId> = resp
            .json()
            .with_context(|| "Parsing Forgejo issue labels")?;
        Ok(labels)
    }

    /// Applies `label_names` to the given issue/PR.
    /// If a label does not yet exist in the repository, it is automatically created
    /// with a default grey colour (`#cccccc`). Callers that pass user-derived label names
    /// should validate them first.
    ///
    /// Ensure labels exist in the repo (create them if absent), then add them to
    /// issue/PR `number` by ID.
    pub fn add_labels(&self, number: u64, labels: &[&str]) -> Result<()> {
        let repo_labels = self.list_repo_labels_with_id()?;
        let mut ids = Vec::new();
        for &name in labels {
            let id = if let Some(l) = repo_labels.iter().find(|l| l.name == name) {
                l.id
            } else {
                // Create the label with a neutral grey color
                self.create_label(name, "#cccccc")?
            };
            ids.push(id);
        }
        let url = self.api_url(&format!(
            "/repos/{}/{}/issues/{}/labels",
            self.owner, self.repo, number
        ));
        let body = AddLabelsByIdBody { labels: ids };
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(&body)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo add_labels failed ({}): {}", status, body);
        }
        Ok(())
    }

    /// Create a label in the repository; returns its new numeric ID.
    fn create_label(&self, name: &str, color: &str) -> Result<u64> {
        #[derive(Deserialize)]
        struct Resp {
            id: u64,
        }
        let url = self.api_url(&format!("/repos/{}/{}/labels", self.owner, self.repo));
        let body = CreateLabelBody { name, color };
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(&body)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!(
                "Forgejo create_label '{}' failed ({}): {}",
                name,
                status,
                body
            );
        }
        let r: Resp = resp.json().context("Parsing create_label response")?;
        Ok(r.id)
    }

    /// Remove a label from issue/PR `number` by name.
    pub fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        let issue_labels = self.get_issue_labels_with_id(number)?;
        let Some(lbl) = issue_labels.iter().find(|l| l.name == label) else {
            return Ok(()); // label not present — nothing to do
        };
        let url = self.api_url(&format!(
            "/repos/{}/{}/issues/{}/labels/{}",
            self.owner, self.repo, number, lbl.id
        ));
        let resp = self
            .http
            .delete(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .with_context(|| format!("DELETE {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo remove_label failed ({}): {}", status, body);
        }
        Ok(())
    }

    /// Close a pull request.
    pub fn close_pr(&self, number: u64) -> Result<()> {
        let url = self.api_url(&format!(
            "/repos/{}/{}/pulls/{}",
            self.owner, self.repo, number
        ));
        let body = PatchPrBody { state: "closed" };
        let resp = self
            .http
            .patch(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(&body)
            .send()
            .with_context(|| format!("PATCH {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo close_pr failed ({}): {}", status, body);
        }
        Ok(())
    }

    /// Return the last `n` issue comments, newest-last.
    pub fn get_last_issue_comments(
        &self,
        number: u64,
        n: usize,
    ) -> Result<Vec<IssueComment>> {
        // Fetches the full comment history in order to return the last `n` entries.
        // For issues with many comments this performs multiple page fetches.
        // A reverse-pagination approach would be more efficient but requires
        // server support for sorting by descending date.
        let mut all = self.get_all_issue_comments(number)?;
        if all.len() > n {
            all.drain(..all.len() - n);
        }
        Ok(all)
    }

    /// Return all issue comments in chronological order.
    pub fn get_all_issue_comments(
        &self,
        number: u64,
    ) -> Result<Vec<IssueComment>> {
        const MAX_PAGES: u32 = 1_000;
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let url = self.api_url(&format!(
                "/repos/{}/{}/issues/{}/comments?page={}&limit=50",
                self.owner, self.repo, number, page
            ));
            let resp = self
                .http
                .get(&url)
                .header("Authorization", format!("token {}", self.token))
                .send()
                .with_context(|| format!("GET {}", url))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().unwrap_or_default();
                anyhow::bail!(
                    "Forgejo get_all_issue_comments failed ({}): {}",
                    status,
                    body
                );
            }
            let comments: Vec<IssueComment> = resp
                .json()
                .with_context(|| "Parsing Forgejo issue comments")?;
            if comments.is_empty() {
                break;
            }
            all.extend(comments);
            page += 1;
            if page > MAX_PAGES {
                eprintln!(
                    "Warning: pagination in get_all_issue_comments exceeded {} pages; results may be incomplete.",
                    MAX_PAGES
                );
                break;
            }
        }
        Ok(all)
    }

    /// Create a PR on Forgejo.  `head` must be in "owner:branch" format.
    pub fn create_pr(
        &self,
        title: &str,
        base: &str,
        head: &str,
        body: &str,
    ) -> Result<PullRequest> {
        let url = self.api_url(&format!("/repos/{}/{}/pulls", self.owner, self.repo));
        let req_body = CreateForgejoPrBody {
            title: title.to_string(),
            head: head.to_string(),
            base: base.to_string(),
            body: body.to_string(),
        };
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(&req_body)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Forgejo create_pr failed ({}): {}", status, body);
        }
        let pr: PullRequest =
            resp.json().with_context(|| "Parsing created Forgejo PR")?;
        Ok(pr)
    }

    pub fn get_authenticated_user_login(&self) -> Result<String> {
        let url = self.api_url("/user");
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .with_context(|| "GET /api/v1/user")?;
        if !resp.status().is_success() {
            let status = resp.status();
            anyhow::bail!(
                "Forgejo authentication failed ({}): check your forgejo-token",
                status
            );
        }
        let user: serde_json::Value = resp.json().with_context(|| "Parsing Forgejo /user")?;
        user["login"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Forgejo /user response missing 'login' field"))
    }

    /// Return inline review comments for a PR.
    /// Forgejo organises these differently (via reviews); returns regular issue
    /// comments as a best-effort fallback.
    pub fn list_review_comments(
        &self,
        pr_number: u64,
    ) -> Result<Vec<ReviewComment>> {
        let _ = pr_number;
        // TODO: Forgejo API does not yet expose per-line review comments
        Ok(vec![])
    }

    /// Post an inline review comment.  Forgejo's review-comment API differs
    /// from GitHub's; we fall back to a regular issue comment here.
    pub fn create_review_comment(
        &self,
        pr_number: u64,
        _commit_id: &str,
        path: &str,
        _line: u64,
        body: &str,
    ) -> Result<()> {
        // Forgejo inline review API is not yet implemented; post as a plain issue comment.
        let text = format!(
            "**Review comment on `{}`** _(posted as issue comment — Forgejo inline review not yet supported)_\n\n{}",
            path, body
        );
        self.comment_issue(pr_number, &text)
    }
}

pub struct ForgejoTicket {
    pub client: std::sync::Arc<ForgejoClient>,
    pub number: u64,
    /// Prefix that marks metadata lines in comments (from config).
    pub comment_field_prefix: String,
    data: std::sync::OnceLock<ForgejoIssue>,
    comments: std::sync::OnceLock<Vec<IssueComment>>,
}

impl ForgejoTicket {
    pub fn new(
        client: std::sync::Arc<ForgejoClient>,
        number: u64,
        comment_field_prefix: String,
    ) -> Self {
        ForgejoTicket {
            client,
            number,
            comment_field_prefix,
            data: std::sync::OnceLock::new(),
            comments: std::sync::OnceLock::new(),
        }
    }

    pub fn data(&self) -> Result<&ForgejoIssue> {
        if let Some(d) = self.data.get() {
            return Ok(d);
        }
        println!("Retrieving issue {}", self.number);
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
        let comments = self.client.get_all_issue_comments(self.number)?;
        let _ = self.comments.set(comments);
        Ok(self
            .comments
            .get()
            .expect("OnceLock was just set above; this is a logic error if None"))
    }

    /// Collect all comment lines matching `<prefix><fieldname>: <value>` and
    /// join them with a space.  Multiple matching lines (e.g. one Bugzilla URL
    /// and one Jira URL on separate lines) are merged so both are visible to
    /// the regex scanners in push.rs.
    fn comment_field(&self, name: &str) -> Result<Option<String>> {
        let prefix = &self.comment_field_prefix;
        let needle = format!("{}:", name);
        let mut values: Vec<String> = Vec::new();

        let mut scan = |text: &str| {
            for line in text.lines() {
                let rest = if prefix.is_empty() {
                    line
                } else {
                    match line.strip_prefix(&**prefix) {
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

        // Scan the issue body first, then all comments.
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

    pub fn reviewer(&self) -> Result<Option<String>> {
        self.comment_field("reviewer")
    }

    pub fn rhbz(&self) -> Result<Option<String>> {
        self.comment_field("rhbz")
    }

    pub fn milestone(&self) -> Result<Option<String>> {
        Ok(self.data()?.milestone_title().map(|s| s.to_string()))
    }

    pub fn title(&self) -> Result<String> {
        Ok(self.data()?.title.clone())
    }

    pub fn is_closed(&self) -> Result<bool> {
        Ok(self.data()?.is_closed())
    }

    pub fn comment(&self, text: &str) -> Result<()> {
        self.client.comment_issue(self.number, text)
    }

    pub fn close(&self) -> Result<()> {
        self.client.close_issue(self.number)
    }
}

impl TicketOps for ForgejoTicket {
    fn number(&self) -> u64 { self.number }
    fn reviewer(&self) -> Result<Option<String>> { self.reviewer() }
    fn rhbz(&self) -> Result<Option<String>> { self.rhbz() }
    fn milestone(&self) -> Result<Option<String>> { self.milestone() }
    fn title(&self) -> Result<String> { self.title() }
    fn is_closed(&self) -> Result<bool> { self.is_closed() }
    fn comment(&self, text: &str) -> Result<()> { self.comment(text) }
    fn close(&self) -> Result<()> { self.close() }
}
