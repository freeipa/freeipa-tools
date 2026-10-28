use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub struct JiraClient {
    pub http: reqwest::blocking::Client,
    token: String,
    pub server: String,
}

#[derive(Debug, Deserialize)]
pub struct JiraTransition {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct TransitionsResponse {
    transitions: Vec<JiraTransition>,
}

#[derive(Serialize)]
struct AddCommentBody<'a> {
    body: &'a str,
}

#[derive(Serialize)]
struct TransitionBody<'a> {
    transition: TransitionRef<'a>,
}

#[derive(Serialize)]
struct TransitionRef<'a> {
    id: &'a str,
}

impl JiraClient {
    pub fn new(server: &str, token: &str) -> Result<Self> {
        use std::time::Duration;
        let http = reqwest::blocking::Client::builder()
            .user_agent("ipatool/1.0")
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build Jira HTTP client")?;
        Ok(JiraClient {
            http,
            token: token.to_string(),
            server: server.trim_end_matches('/').to_string(),
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}/rest/api/2{}", self.server, path)
    }

    /// Fetch summary, description and comment bodies of an issue.
    pub fn get_issue_text(&self, issue_key: &str) -> Result<(String, String, Vec<String>)> {
        let url = self.api_url(&format!("/issue/{}", issue_key));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .with_context(|| format!("GET {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Jira get_issue failed ({}): {}", status, body);
        }
        let data: serde_json::Value = resp
            .json()
            .with_context(|| format!("Parsing jira issue {}", issue_key))?;
        let fields = &data["fields"];
        let mut comments = Vec::new();
        if let Some(list) = fields
            .get("comment")
            .and_then(|c| c.get("comments"))
            .and_then(|c| c.as_array())
        {
            for c in list {
                if let Some(body) = c["body"].as_str() {
                    comments.push(body.to_string());
                }
            }
        }
        Ok((
            fields["summary"].as_str().unwrap_or("").to_string(),
            fields["description"].as_str().unwrap_or("").to_string(),
            comments,
        ))
    }

    /// Create an issue in `project_key`; returns the new issue key.
    pub fn create_issue(
        &self,
        project_key: &str,
        issuetype: &str,
        summary: &str,
        description: &str,
    ) -> Result<String> {
        let url = self.api_url("/issue");
        let body = serde_json::json!({
            "fields": {
                "project": { "key": project_key },
                "issuetype": { "name": issuetype },
                "summary": summary,
                "description": description,
            }
        });
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Jira create_issue failed ({}): {}", status, body);
        }
        let data: serde_json::Value = resp.json().context("Parsing jira create_issue response")?;
        data["key"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Jira create_issue response has no key"))
    }

    pub fn add_comment(&self, issue_key: &str, text: &str) -> Result<()> {
        let url = self.api_url(&format!("/issue/{}/comment", issue_key));
        let body = AddCommentBody { body: text };
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Jira add_comment failed ({}): {}", status, body);
        }
        Ok(())
    }

    pub fn get_transitions(&self, issue_key: &str) -> Result<Vec<JiraTransition>> {
        let url = self.api_url(&format!("/issue/{}/transitions", issue_key));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .with_context(|| format!("GET {}", url))?;
        let data: TransitionsResponse = resp
            .json()
            .with_context(|| format!("Parsing jira transitions for {}", issue_key))?;
        Ok(data.transitions)
    }

    pub fn transition_issue(&self, issue_key: &str, transition_id: &str) -> Result<()> {
        let url = self.api_url(&format!("/issue/{}/transitions", issue_key));
        let body = TransitionBody {
            transition: TransitionRef { id: transition_id },
        };
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .with_context(|| format!("POST {}", url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Jira transition_issue failed ({}): {}", status, body);
        }
        Ok(())
    }
}
