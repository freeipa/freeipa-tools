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
