use anyhow::{bail, Context, Result};
use serde_json::json;

/// Minimal Bugzilla REST client (`/restapi/v1`) for reading and creating bugs.
pub struct BugzillaClient {
    pub http: reqwest::blocking::Client,
    api_key: String,
    pub base_url: String,
}

impl BugzillaClient {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self> {
        use std::time::Duration;
        let http = reqwest::blocking::Client::builder()
            .user_agent("ipatool/1.0")
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build Bugzilla HTTP client")?;
        Ok(BugzillaClient {
            http,
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}/restapi/v1{}", self.base_url, path)
    }

    /// Fetch a bug's summary, description (comment 0) and comments.
    /// Returns `(summary, description, comments_from_1_on)`.
    pub fn get_bug(&self, id: u64) -> Result<(String, String, Vec<String>)> {
        let url = reqwest::Url::parse_with_params(
            &self.api_url(&format!("/bug/{}", id)),
            &[("api_key", self.api_key.as_str())],
        )
        .with_context(|| "building Bugzilla bug URL")?
        .to_string();
        let resp = self
            .http
            .get(&url)
            .send()
            .with_context(|| format!("GET {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Bugzilla get_bug failed ({}): {}", status, body);
        }
        let data: serde_json::Value = resp
            .json()
            .with_context(|| format!("Parsing Bugzilla bug {}", id))?;
        let bugs = data
            .get("bugs")
            .and_then(|b| b.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| anyhow::anyhow!("Bugzilla response for bug {} has no bug object", id))?;
        let summary = bugs["summary"].as_str().unwrap_or("").to_string();
        let description = bugs["description"].as_str().unwrap_or("").to_string();
        let mut comments = Vec::new();
        if let Some(list) = bugs.get("comments").and_then(|c| c.as_array()) {
            for c in list {
                // Comment 0 duplicates the description; skip it.
                if c["count"].as_u64().unwrap_or(0) == 0 {
                    continue;
                }
                if let Some(text) = c["text"].as_str() {
                    comments.push(text.to_string());
                }
            }
        }

        Ok((summary, description, comments))
    }

    /// Create a bug in `product` with optional `component`; returns the new bug id.
    pub fn create_bug(
        &self,
        product: &str,
        component: &str,
        summary: &str,
        description: &str,
    ) -> Result<u64> {
        let url = reqwest::Url::parse_with_params(
            &self.api_url("/bug"),
            &[("api_key", self.api_key.as_str())],
        )
        .with_context(|| "building Bugzilla create URL")?
        .to_string();
        let mut payload =
            json!({ "product": product, "summary": summary, "description": description });
        if !component.is_empty() {
            payload["component"] = json!(component);
        }
        let resp = self
            .http
            .post(&url)
            .json(&payload)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Bugzilla create_bug failed ({}): {}", status, body);
        }
        let data: serde_json::Value = resp
            .json()
            .context("Parsing Bugzilla create_bug response")?;
        data["id"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("Bugzilla create_bug response has no id"))
    }
}
