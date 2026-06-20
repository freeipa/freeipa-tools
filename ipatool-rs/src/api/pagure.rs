use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

use crate::api::types::TicketOps;

pub struct PagureClient {
    pub http: reqwest::blocking::Client,
    token: String,
    pub repository: String,
    pub base_url: String,
}

#[derive(Debug, Deserialize)]
pub struct CustomField {
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PagureIssue {
    #[serde(default)]
    pub id: u64,
    pub title: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub content: String,
    pub status: String,
    #[serde(default)]
    pub close_status: Option<String>,
    #[serde(default)]
    pub milestone: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub custom_fields: Vec<CustomField>,
}

impl PagureIssue {
    pub fn custom_field(&self, name: &str) -> Option<&str> {
        for f in &self.custom_fields {
            if f.name == name {
                return f.value.as_deref();
            }
        }
        None
    }

    pub fn reviewer(&self) -> Option<&str> {
        self.custom_field("reviewer")
    }

    pub fn rhbz(&self) -> Option<&str> {
        self.custom_field("rhbz")
    }

    pub fn is_closed(&self) -> bool {
        self.status == "Closed"
    }
}

impl PagureClient {
    pub fn new(token: &str, repository: &str) -> Result<Self> {
        use std::time::Duration;
        let http = reqwest::blocking::Client::builder()
            .user_agent("ipatool/1.0")
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build Pagure HTTP client")?;
        Ok(PagureClient {
            http,
            token: token.to_string(),
            repository: repository.to_string(),
            base_url: "https://pagure.io/api/0".to_string(),
        })
    }

    pub fn get_issue(&self, number: u64) -> Result<PagureIssue> {
        let url = format!("{}/{}/issue/{}", self.base_url, self.repository, number);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .header("Accept", "*/*")
            .send()
            .with_context(|| format!("GET {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Pagure get_issue failed ({}): {}", status, body);
        }
        let issue: PagureIssue = resp
            .json()
            .with_context(|| format!("Parsing pagure issue {}", number))?;
        Ok(issue)
    }

    pub fn list_issues(&self, status: &str, milestones: &[&str]) -> Result<Vec<PagureIssue>> {
        #[derive(Deserialize)]
        struct IssueList {
            issues: Vec<PagureIssue>,
            #[allow(dead_code)]
            total_issues: u64,
        }

        const MAX_PAGES: u32 = 1_000;
        let milestones_param = milestones.join(",");
        let mut all_issues = Vec::new();
        let mut page = 1u32;
        loop {
            if page > MAX_PAGES {
                eprintln!(
                    "Warning: pagination in list_issues exceeded {} pages, results may be incomplete",
                    MAX_PAGES
                );
                break;
            }
            let base = format!("{}/{}/issues", self.base_url, self.repository);
            let url = reqwest::Url::parse_with_params(
                &base,
                &[
                    ("status", status),
                    ("milestones", &milestones_param),
                    ("per_page", "100"),
                    ("page", &page.to_string()),
                ],
            )
            .with_context(|| {
                format!(
                    "building Pagure issue list URL for milestone(s) '{}'",
                    milestones_param
                )
            })?;
            let url = url.to_string();
            let resp = self
                .http
                .get(&url)
                .header("Authorization", format!("token {}", self.token))
                .header("Accept", "*/*")
                .send()
                .with_context(|| format!("GET {}", url))?;
            if !resp.status().is_success() {
                let status_code = resp.status();
                let body = resp.text().unwrap_or_default();
                bail!("Pagure list_issues failed ({}): {}", status_code, body);
            }
            let list: IssueList = resp
                .json()
                .with_context(|| format!("Parsing pagure issue list page {}", page))?;
            if list.issues.is_empty() {
                break;
            }
            all_issues.extend(list.issues);
            page += 1;
        }
        Ok(all_issues)
    }

    pub fn create_issue(&self, title: &str, body: &str) -> Result<u64> {
        #[derive(serde::Deserialize)]
        struct Resp {
            issue: RespIssue,
        }
        #[derive(serde::Deserialize)]
        struct RespIssue {
            id: u64,
        }

        let url = format!("{}/{}/new_issue", self.base_url, self.repository);
        let mut form = HashMap::new();
        form.insert("title", title);
        form.insert("issue_content", body);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("token {}", self.token))
            .form(&form)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Pagure create_issue failed ({}): {}", status, body);
        }
        let r: Resp = resp.json().context("Parsing create_issue response")?;
        Ok(r.issue.id)
    }

    pub fn comment_issue(&self, number: u64, text: &str) -> Result<()> {
        let url = format!(
            "{}/{}/issue/{}/comment",
            self.base_url, self.repository, number
        );
        let mut form = HashMap::new();
        form.insert("comment", text);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("token {}", self.token))
            .form(&form)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Pagure comment failed ({}): {}", status, body);
        }
        Ok(())
    }

    pub fn close_issue(&self, number: u64) -> Result<()> {
        let url = format!(
            "{}/{}/issue/{}/status",
            self.base_url, self.repository, number
        );
        let mut form = HashMap::new();
        form.insert("status", "Closed");
        form.insert("close_status", "fixed");
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("token {}", self.token))
            .form(&form)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Pagure close issue failed ({}): {}", status, body);
        }
        Ok(())
    }
}

pub struct PagureTicket {
    pub client: std::sync::Arc<PagureClient>,
    pub number: u64,
    data: std::sync::OnceLock<PagureIssue>,
}

impl PagureTicket {
    pub fn new(client: std::sync::Arc<PagureClient>, number: u64) -> Self {
        PagureTicket {
            client,
            number,
            data: std::sync::OnceLock::new(),
        }
    }

    pub fn data(&self) -> Result<&PagureIssue> {
        if let Some(d) = self.data.get() {
            return Ok(d);
        }
        println!("Retrieving ticket {}", self.number);
        let issue = self.client.get_issue(self.number)?;
        let _ = self.data.set(issue);
        Ok(self
            .data
            .get()
            .expect("OnceLock was just set above; this is a logic error if None"))
    }

    pub fn reviewer(&self) -> Result<Option<String>> {
        Ok(self.data()?.reviewer().map(|s| s.to_string()))
    }

    pub fn rhbz(&self) -> Result<Option<String>> {
        Ok(self.data()?.rhbz().map(|s| s.to_string()))
    }

    pub fn milestone(&self) -> Result<Option<String>> {
        Ok(self.data()?.milestone.clone())
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

impl TicketOps for PagureTicket {
    fn number(&self) -> u64 {
        self.number
    }
    fn reviewer(&self) -> Result<Option<String>> {
        self.reviewer()
    }
    fn rhbz(&self) -> Result<Option<String>> {
        self.rhbz()
    }
    fn milestone(&self) -> Result<Option<String>> {
        self.milestone()
    }
    fn title(&self) -> Result<String> {
        self.title()
    }
    fn is_closed(&self) -> Result<bool> {
        self.is_closed()
    }
    fn comment(&self, text: &str) -> Result<()> {
        self.comment(text)
    }
    fn close(&self) -> Result<()> {
        self.close()
    }
}
