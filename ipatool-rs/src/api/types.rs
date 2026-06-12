use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Label {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GitRef {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub html_url: String,
    pub head: GitRef,
    pub base: GitRef,
    pub user: User,
    pub mergeable: Option<bool>,
    pub merged: Option<bool>,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub commits: Option<u64>,
    #[serde(default)]
    pub comments: Option<u64>,
    #[serde(default)]
    pub review_comments: Option<u64>,
    #[serde(default)]
    pub additions: Option<u64>,
    #[serde(default)]
    pub deletions: Option<u64>,
    #[serde(default)]
    pub changed_files: Option<u64>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl PullRequest {
    pub fn is_merged(&self) -> bool {
        self.merged.unwrap_or(false)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct IssueComment {
    pub id: u64,
    pub user: User,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PrFile {
    pub filename: String,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub changes: u64,
    #[serde(default)]
    pub previous_filename: Option<String>,
    /// Unified diff patch for this file (absent for binary / very large files).
    #[serde(default)]
    pub patch: Option<String>,
}

/// A single pull-request review comment (line-level).
#[derive(Debug, Deserialize, Clone)]
pub struct ReviewComment {
    pub user: User,
    pub body: String,
    pub path: String,
    /// Line number in the new file (RIGHT side). Absent on some legacy comments.
    pub line: Option<u64>,
    /// Line number in the original file (LEFT side).
    pub original_line: Option<u64>,
    /// "LEFT" or "RIGHT".
    pub side: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitParent {
    pub sha: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitMessage {
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Commit {
    pub sha: String,
    pub commit: CommitMessage,
    pub parents: Vec<CommitParent>,
}

/// Per-context CI job status, provider-agnostic.
/// GitHub populates `url` from `target_url`; other providers map their
/// equivalent field here so the rest of the code stays provider-independent.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CiStatus {
    pub state: String,
    pub url: Option<String>,
}

/// Sort commits topologically (linear chain, no merges).
pub fn sorted_commits(commits: Vec<Commit>) -> Result<Vec<Commit>> {
    for c in &commits {
        if c.parents.len() != 1 {
            anyhow::bail!(
                "Commit {} has {} parents (merge commit not supported)",
                c.sha,
                c.parents.len()
            );
        }
    }

    let commit_ids: HashSet<&str> = commits.iter().map(|c| c.sha.as_str()).collect();

    // Find root commit (parent not in set)
    let first = commits
        .iter()
        .find(|c| !commit_ids.contains(c.parents[0].sha.as_str()))
        .ok_or_else(|| anyhow::anyhow!("No first commit found in chain"))?;

    let mut result = vec![first.clone()];
    let mut parent_id = first.sha.clone();

    while result.len() < commits.len() {
        let next = commits
            .iter()
            .find(|c| c.parents[0].sha == parent_id)
            .ok_or_else(|| {
                anyhow::anyhow!("Commit {} should have child but none found", parent_id)
            })?;
        parent_id = next.sha.clone();
        result.push(next.clone());
    }

    Ok(result)
}

/// Format labels with terminal 24-bit color codes.
pub fn labels_colorize(labels: &[Label], color_enabled: bool) -> String {
    let parts: Vec<String> = labels
        .iter()
        .map(|l| {
            if color_enabled && l.color.len() >= 6 {
                if let Ok(r) = u8::from_str_radix(&l.color[0..2], 16) {
                    if let Ok(g) = u8::from_str_radix(&l.color[2..4], 16) {
                        if let Ok(b) = u8::from_str_radix(&l.color[4..6], 16) {
                            return format!("\x1b[38;2;{};{};{}m{}\x1b[0m", r, g, b, l.name);
                        }
                    }
                }
            }
            l.name.clone()
        })
        .collect();
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_commit(sha: &str, parent_sha: &str, message: &str) -> Commit {
        Commit {
            sha: sha.to_string(),
            commit: CommitMessage {
                message: message.to_string(),
            },
            parents: vec![CommitParent {
                sha: parent_sha.to_string(),
            }],
        }
    }

    // ── PullRequest deserialization ───────────────────────────────────────────

    #[test]
    fn test_pr_deserialization_minimal() {
        let json = r#"{
            "number": 42,
            "title": "Fix something",
            "state": "open",
            "html_url": "https://github.com/test/repo/pull/42",
            "head": {"ref": "feature", "sha": "abc123"},
            "base": {"ref": "master", "sha": "def456"},
            "user": {"login": "testuser"}
        }"#;
        let pr: PullRequest = serde_json::from_str(json).unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.title, "Fix something");
        assert_eq!(pr.state, "open");
        assert!(pr.labels.is_empty());
        assert!(pr.merged.is_none());
        assert!(pr.updated_at.is_none());
        assert!(pr.body.is_none());
    }

    #[test]
    fn test_pr_deserialization_full() {
        let json = r#"{
            "number": 99,
            "title": "Full PR",
            "state": "closed",
            "html_url": "https://github.com/org/repo/pull/99",
            "head": {"ref": "my-feature", "sha": "deadbeef"},
            "base": {"ref": "master", "sha": "cafebabe"},
            "user": {"login": "contributor"},
            "mergeable": true,
            "merged": true,
            "labels": [{"name": "ack", "color": "00ff00"}],
            "body": "PR description",
            "commits": 3,
            "additions": 50,
            "deletions": 10,
            "changed_files": 4,
            "updated_at": "2024-06-01T12:00:00Z"
        }"#;
        let pr: PullRequest = serde_json::from_str(json).unwrap();
        assert_eq!(pr.number, 99);
        assert!(pr.is_merged());
        assert_eq!(pr.labels.len(), 1);
        assert_eq!(pr.labels[0].name, "ack");
        assert_eq!(pr.updated_at.as_deref(), Some("2024-06-01T12:00:00Z"));
        assert_eq!(pr.additions, Some(50));
    }

    #[test]
    fn test_pr_is_merged_absent_defaults_false() {
        let json = r#"{"number":1,"title":"t","state":"open","html_url":"u","head":{"ref":"r","sha":"s"},"base":{"ref":"m","sha":"b"},"user":{"login":"u"}}"#;
        let pr: PullRequest = serde_json::from_str(json).unwrap();
        assert!(!pr.is_merged());
    }

    #[test]
    fn test_pr_serialization_roundtrip() {
        let json = r#"{"number":42,"title":"Test","state":"open","html_url":"https://github.com/t/r/pull/42","head":{"ref":"feature","sha":"abc"},"base":{"ref":"master","sha":"def"},"user":{"login":"u"},"mergeable":null,"merged":null,"labels":[],"body":null,"commits":null,"comments":null,"review_comments":null,"additions":null,"deletions":null,"changed_files":null,"updated_at":null}"#;
        let pr: PullRequest = serde_json::from_str(json).unwrap();
        let re_json = serde_json::to_string(&pr).unwrap();
        let pr2: PullRequest = serde_json::from_str(&re_json).unwrap();
        assert_eq!(pr2.number, 42);
        assert_eq!(pr2.title, "Test");
    }

    // ── PrFile deserialization ────────────────────────────────────────────────

    #[test]
    fn test_file_deserialization_with_rename() {
        let json = r#"{
            "filename": "src/new_name.rs",
            "status": "renamed",
            "additions": 0,
            "deletions": 0,
            "changes": 0,
            "previous_filename": "src/old_name.rs",
            "patch": "@@ -1 +1 @@\n-old\n+new"
        }"#;
        let file: PrFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.previous_filename.as_deref(), Some("src/old_name.rs"));
        assert_eq!(file.status, "renamed");
        assert!(file.patch.is_some());
    }

    #[test]
    fn test_file_deserialization_no_patch() {
        let json = r#"{
            "filename": "binary.bin",
            "status": "added",
            "additions": 0,
            "deletions": 0,
            "changes": 0
        }"#;
        let file: PrFile = serde_json::from_str(json).unwrap();
        assert!(file.patch.is_none());
        assert!(file.previous_filename.is_none());
    }

    // ── IssueComment deserialization ──────────────────────────────────────────

    #[test]
    fn test_comment_deserialization() {
        let json = r#"{
            "id": 123,
            "user": {"login": "reviewer"},
            "body": "LGTM!",
            "created_at": "2024-01-15T09:00:00Z"
        }"#;
        let comment: IssueComment = serde_json::from_str(json).unwrap();
        assert_eq!(comment.id, 123);
        assert_eq!(comment.user.login, "reviewer");
        assert_eq!(comment.body, "LGTM!");
    }

    #[test]
    fn test_comment_serialization_roundtrip() {
        let comment = IssueComment {
            id: 1,
            user: User {
                login: "tester".to_string(),
            },
            body: "hello".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&comment).unwrap();
        let c2: IssueComment = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.body, "hello");
    }

    // ── sorted_commits ────────────────────────────────────────────────────────

    #[test]
    fn test_sorted_commits_single() {
        let c = make_commit("sha1", "outside_root", "Only commit");
        let sorted = sorted_commits(vec![c]).unwrap();
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].sha, "sha1");
    }

    #[test]
    fn test_sorted_commits_already_ordered() {
        let c1 = make_commit("sha1", "base", "First");
        let c2 = make_commit("sha2", "sha1", "Second");
        let sorted = sorted_commits(vec![c1, c2]).unwrap();
        assert_eq!(sorted[0].sha, "sha1");
        assert_eq!(sorted[1].sha, "sha2");
    }

    #[test]
    fn test_sorted_commits_reversed_input() {
        let c1 = make_commit("sha1", "base", "First");
        let c2 = make_commit("sha2", "sha1", "Second");
        let sorted = sorted_commits(vec![c2, c1]).unwrap();
        assert_eq!(sorted[0].sha, "sha1");
        assert_eq!(sorted[1].sha, "sha2");
    }

    #[test]
    fn test_sorted_commits_three() {
        let c1 = make_commit("a", "root", "First");
        let c2 = make_commit("b", "a", "Second");
        let c3 = make_commit("c", "b", "Third");
        let sorted = sorted_commits(vec![c3, c1, c2]).unwrap();
        assert_eq!(sorted[0].sha, "a");
        assert_eq!(sorted[1].sha, "b");
        assert_eq!(sorted[2].sha, "c");
    }

    #[test]
    fn test_sorted_commits_merge_commit_fails() {
        let merge = Commit {
            sha: "merge_sha".to_string(),
            commit: CommitMessage {
                message: "Merge branch".to_string(),
            },
            parents: vec![
                CommitParent {
                    sha: "p1".to_string(),
                },
                CommitParent {
                    sha: "p2".to_string(),
                },
            ],
        };
        let result = sorted_commits(vec![merge]);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("2 parents") || msg.contains("parents"));
    }

    // ── labels_colorize ───────────────────────────────────────────────────────

    #[test]
    fn test_labels_colorize_no_color() {
        let labels = vec![
            Label {
                name: "ack".to_string(),
                color: "00ff00".to_string(),
            },
            Label {
                name: "pushed".to_string(),
                color: "ff0000".to_string(),
            },
        ];
        let result = labels_colorize(&labels, false);
        assert_eq!(result, "ack,pushed");
    }

    #[test]
    fn test_labels_colorize_with_color() {
        let labels = vec![Label {
            name: "ack".to_string(),
            color: "00ff00".to_string(),
        }];
        let result = labels_colorize(&labels, true);
        assert!(result.contains("\x1b[38;2;"));
        assert!(result.contains("ack"));
        assert!(result.contains("\x1b[0m"));
    }

    #[test]
    fn test_labels_colorize_empty() {
        let result = labels_colorize(&[], false);
        assert!(result.is_empty());
    }

    #[test]
    fn test_labels_colorize_single_no_comma() {
        let labels = vec![Label {
            name: "ack".to_string(),
            color: "00ff00".to_string(),
        }];
        let result = labels_colorize(&labels, false);
        assert!(!result.contains(','));
        assert_eq!(result, "ack");
    }
}
