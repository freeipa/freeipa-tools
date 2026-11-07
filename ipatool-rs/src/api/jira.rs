use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub struct JiraClient {
    pub http: reqwest::blocking::Client,
    token: String,
    username: Option<String>,
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
        Self::build(server, token, None)
    }

    pub fn new_with_username(server: &str, username: &str, token: &str) -> Result<Self> {
        Self::build(server, token, Some(username.to_string()))
    }

    fn build(server: &str, token: &str, username: Option<String>) -> Result<Self> {
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
            username,
            server: server.trim_end_matches('/').to_string(),
        })
    }

    fn authorize(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        if let Some(username) = &self.username {
            request.basic_auth(username, Some(&self.token))
        } else {
            request.bearer_auth(&self.token)
        }
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}/rest/api/2{}", self.server, path)
    }

    /// Fetch the canonical key, summary, description and comment bodies of an issue.
    pub fn get_issue_text(&self, issue_key: &str) -> Result<(String, String, String, Vec<String>)> {
        let url = self.api_url(&format!("/issue/{}", issue_key));
        let resp = self
            .authorize(self.http.get(&url))
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
            data["key"]
                .as_str()
                .filter(|key| !key.is_empty())
                .ok_or_else(|| anyhow::anyhow!("Jira issue response has no key"))?
                .to_string(),
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
            .authorize(self.http.post(&url))
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
            .authorize(self.http.post(&url))
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
            .authorize(self.http.get(&url))
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
            .authorize(self.http.post(&url))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn basic_and_bearer_authenticate_jira_reads_and_creates() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            for (method, authorization, response_body) in [
                (
                    "GET",
                    "Basic dXNlckBleGFtcGxlLmNvbTphcGlfdG9rZW4=",
                    r#"{"key":"RHEL-1","fields":{"summary":"Issue","description":"Body"}}"#,
                ),
                (
                    "POST",
                    "Basic dXNlckBleGFtcGxlLmNvbTphcGlfdG9rZW4=",
                    r#"{"key":"RHEL-2"}"#,
                ),
                (
                    "GET",
                    "Bearer api_token",
                    r#"{"key":"RHEL-1","fields":{"summary":"Issue","description":"Body"}}"#,
                ),
                ("POST", "Bearer api_token", r#"{"key":"RHEL-2"}"#),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with(method), "{line}");
                let mut auth = None;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if line.to_ascii_lowercase().starts_with("authorization: ") {
                        auth = Some(line.trim().split_once(": ").unwrap().1.to_string());
                    }
                }
                assert_eq!(auth.as_deref(), Some(authorization));
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                )
                .unwrap();
            }
        });

        let basic =
            JiraClient::new_with_username(&server, "user@example.com", "api_token").unwrap();
        let bearer = JiraClient::new(&server, "api_token").unwrap();
        assert_eq!(basic.get_issue_text("RHEL-1").unwrap().0, "RHEL-1");
        assert_eq!(
            basic.create_issue("RHEL", "Bug", "Issue", "Body").unwrap(),
            "RHEL-2"
        );
        assert_eq!(bearer.get_issue_text("RHEL-1").unwrap().0, "RHEL-1");
        assert_eq!(
            bearer.create_issue("RHEL", "Bug", "Issue", "Body").unwrap(),
            "RHEL-2"
        );
        worker.join().unwrap();
    }
}
