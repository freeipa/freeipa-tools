use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

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
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub content: String,
    pub status: String,
    #[serde(default)]
    pub close_status: Option<String>,
    #[serde(default)]
    pub milestone: Option<String>,
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

    pub fn is_fixed(&self) -> bool {
        self.is_closed()
            && self
                .close_status
                .as_deref()
                .map(|s| s == "fixed")
                .unwrap_or(false)
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
        Ok(self.data.get().unwrap())
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
