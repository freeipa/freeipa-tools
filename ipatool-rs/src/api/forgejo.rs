use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub struct ForgejoClient {
    pub http: reqwest::blocking::Client,
    token: String,
    pub base_url: String,
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Deserialize)]
pub struct ForgejoMilestone {
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct ForgejoIssue {
    pub number: u64,
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
        let url = self.api_url(&format!("/repos/{}/{}/issues/{}", self.owner, self.repo, number));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .with_context(|| format!("GET {}", url))?;
        let issue: ForgejoIssue = resp
            .json()
            .with_context(|| format!("Parsing forgejo issue {}", number))?;
        Ok(issue)
    }

    pub fn comment_issue(&self, number: u64, text: &str) -> Result<()> {
        let url =
            self.api_url(&format!("/repos/{}/{}/issues/{}/comments", self.owner, self.repo, number));
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
        let url =
            self.api_url(&format!("/repos/{}/{}/issues/{}", self.owner, self.repo, number));
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
}

pub struct ForgejoTicket {
    pub client: std::sync::Arc<ForgejoClient>,
    pub number: u64,
    data: std::sync::OnceLock<ForgejoIssue>,
}

impl ForgejoTicket {
    pub fn new(client: std::sync::Arc<ForgejoClient>, number: u64) -> Self {
        ForgejoTicket {
            client,
            number,
            data: std::sync::OnceLock::new(),
        }
    }

    pub fn data(&self) -> Result<&ForgejoIssue> {
        if let Some(d) = self.data.get() {
            return Ok(d);
        }
        println!("Retrieving issue {}", self.number);
        let issue = self.client.get_issue(self.number)?;
        let _ = self.data.set(issue);
        Ok(self.data.get().unwrap())
    }

    pub fn reviewer(&self) -> Option<String> {
        None // Forgejo has no custom fields
    }

    pub fn rhbz(&self) -> Option<String> {
        None // Forgejo has no custom fields
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
