use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::api::github::{GitHubComment, GitHubCommit, GitHubFile, GitHubPR};

// ── Provider identifier ───────────────────────────────────────────────────────

/// Identifies which forge backend an action or cached PR belongs to.
/// Adding a new variant here is the only change needed to add a new provider.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    GitHub,
    Forgejo,
    /// Pagure is supported as an issue-tracker backend but has no offline
    /// action-queue support; actions targeting Pagure must be executed online.
    Pagure,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::GitHub => "github",
            Provider::Forgejo => "forgejo",
            Provider::Pagure => "pagure",
        }
    }
}

// ── Cached PR details ─────────────────────────────────────────────────────────

/// The supplementary per-PR data fetched in the background (CI statuses,
/// recent comments, changed files, commits).  Stored separately from the PR
/// list so the two can be refreshed independently.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CachedPrDetails {
    pub statuses: HashMap<String, String>,
    pub comments: Vec<GitHubComment>,
    pub files: Vec<GitHubFile>,
    /// PR commits in topological order (oldest first).
    /// `#[serde(default)]` ensures old cached records (before this field was added)
    /// deserialize without error.
    #[serde(default)]
    pub commits: Vec<GitHubCommit>,
    /// Job result URLs keyed by CI context name (same keys as `statuses`).
    /// Added later; empty on old cache entries.
    #[serde(default)]
    pub status_urls: HashMap<String, String>,
}

// ── Queued mutations ──────────────────────────────────────────────────────────

/// Provider-agnostic mutation variants.  Each variant uses the provider-neutral
/// concept of a "PR number"; the concrete API call is chosen at sync time by
/// dispatching on the wrapping `ProviderAction::provider` field.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum QueuedAction {
    AddLabel {
        pr_number: u64,
        label: String,
    },
    RemoveLabel {
        pr_number: u64,
        label: String,
    },
    PostComment {
        pr_number: u64,
        body: String,
    },
    /// `commit_id`/`path`/`line` are GitHub-specific; Forgejo equivalents would
    /// use the same fields mapped to their API.
    PostReviewComment {
        pr_number: u64,
        commit_id: String,
        path: String,
        line: u64,
        body: String,
    },
    Ack {
        pr_number: u64,
        comment: Option<String>,
    },
    Reject {
        pr_number: u64,
        comment: String,
    },
    UpdateLabels {
        pr_number: u64,
        to_add: Vec<String>,
        to_remove: Vec<String>,
    },
}

/// A `QueuedAction` tagged with its origin provider so the sync loop can
/// dispatch to the right API client without inspecting the action itself.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProviderAction {
    pub provider: Provider,
    pub action: QueuedAction,
}

pub struct PendingAction {
    pub id: i64,
    pub provider_action: ProviderAction,
}

// ── Database ──────────────────────────────────────────────────────────────────

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        // Ensure parent directory exists.
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Creating directory {}", parent.display()))?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("Opening SQLite DB at {}", path))?;
        let db = Database {
            conn: Mutex::new(conn),
        };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // The action queue is never dropped — it holds persistent offline work.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS queued_actions (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 action_json TEXT    NOT NULL,
                 created_at  INTEGER NOT NULL
             );",
        )
        .context("Creating queued_actions table")?;

        // Cache tables are keyed by (number, profile) so that different
        // --profile values (which may point to different forges or repos) keep
        // completely separate namespaces.  The default profile is ''.
        //
        // Migration: old databases used (number, provider) as the primary key.
        // Detect this by checking whether the 'profile' column exists.  If it
        // does not, the cache tables are stale; drop and recreate them (cache
        // loss is acceptable — run `cache-update` to repopulate).
        let has_profile: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('prs') WHERE name='profile'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0;

        if !has_profile {
            conn.execute_batch("DROP TABLE IF EXISTS prs; DROP TABLE IF EXISTS pr_details;")
                .context("Dropping old-schema cache tables")?;
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS prs (
                 number     INTEGER NOT NULL,
                 profile    TEXT    NOT NULL DEFAULT '',
                 state      TEXT    NOT NULL,
                 data_json  TEXT    NOT NULL,
                 cached_at  INTEGER NOT NULL,
                 PRIMARY KEY (number, profile)
             );
             CREATE TABLE IF NOT EXISTS pr_details (
                 number        INTEGER NOT NULL,
                 profile       TEXT    NOT NULL DEFAULT '',
                 details_json  TEXT    NOT NULL,
                 pr_updated_at TEXT,
                 cached_at     INTEGER NOT NULL,
                 PRIMARY KEY (number, profile)
             );",
        )
        .context("Creating cache tables")?;

        Ok(())
    }

    // ── PR cache ──────────────────────────────────────────────────────────────

    /// Cache a slice of PRs under the given profile namespace.
    /// The profile is the active `--profile` name (empty string for the default
    /// profile), so that different profile configurations keep separate caches.
    pub fn cache_prs(&self, profile: &str, prs: &[GitHubPR]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_secs();
        for pr in prs {
            let json = serde_json::to_string(pr)?;
            conn.execute(
                "INSERT OR REPLACE INTO prs (number, profile, state, data_json, cached_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![pr.number as i64, profile, pr.state, json, now],
            )?;
        }
        Ok(())
    }

    /// Load cached PRs for the given profile, optionally filtered by state.
    pub fn load_prs(&self, profile: &str, state_filter: &str) -> Result<Vec<GitHubPR>> {
        let conn = self.conn.lock().unwrap();
        let jsons: Vec<String> = if state_filter == "all" {
            let mut stmt =
                conn.prepare("SELECT data_json FROM prs WHERE profile = ?1 ORDER BY number DESC")?;
            let collected: rusqlite::Result<Vec<String>> =
                stmt.query_map([profile], |row| row.get(0))?.collect();
            collected?
        } else {
            let mut stmt = conn.prepare(
                "SELECT data_json FROM prs \
                  WHERE profile = ?1 AND state = ?2 \
                  ORDER BY number DESC",
            )?;
            let collected: rusqlite::Result<Vec<String>> = stmt
                .query_map(params![profile, state_filter], |row| row.get(0))?
                .collect();
            collected?
        };
        jsons
            .iter()
            .map(|j| serde_json::from_str(j).map_err(anyhow::Error::from))
            .collect()
    }

    // ── PR details cache ──────────────────────────────────────────────────────

    /// Store supplementary PR details (CI statuses, comments, files) under
    /// the given profile namespace.
    /// `pr_updated_at` is the `updated_at` timestamp from the forge API; it is
    /// stored so callers can detect unchanged PRs without re-fetching.
    pub fn cache_pr_details(
        &self,
        profile: &str,
        pr_number: u64,
        details: &CachedPrDetails,
        pr_updated_at: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let json = serde_json::to_string(details)?;
        conn.execute(
            "INSERT OR REPLACE INTO pr_details
                 (number, profile, details_json, pr_updated_at, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![pr_number as i64, profile, json, pr_updated_at, now_secs()],
        )?;
        Ok(())
    }

    /// Load cached supplementary details for a PR, or `None` if not cached.
    pub fn load_pr_details(
        &self,
        profile: &str,
        pr_number: u64,
    ) -> Result<Option<CachedPrDetails>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT details_json FROM pr_details WHERE number = ?1 AND profile = ?2",
            params![pr_number as i64, profile],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(json) => Ok(Some(serde_json::from_str(&json)?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Return the `pr_updated_at` value stored when details were last cached,
    /// or `None` if no details have been cached for this PR.
    pub fn pr_details_updated_at(&self, profile: &str, pr_number: u64) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT pr_updated_at FROM pr_details WHERE number = ?1 AND profile = ?2",
            params![pr_number as i64, profile],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    // ── Action queue ──────────────────────────────────────────────────────────

    /// Enqueue a provider-tagged action for later sync.
    pub fn queue_action(&self, action: &ProviderAction) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let json = serde_json::to_string(action)?;
        conn.execute(
            "INSERT INTO queued_actions (action_json, created_at) VALUES (?1, ?2)",
            params![json, now_secs()],
        )?;
        Ok(())
    }

    pub fn pending_actions(&self) -> Result<Vec<PendingAction>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, action_json FROM queued_actions ORDER BY id")?;
        let collected: rusqlite::Result<Vec<(i64, String)>> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect();
        let rows = collected?;
        rows.iter()
            .map(|(id, json)| {
                // Try to deserialize as the new ProviderAction format.  Fall back
                // to plain QueuedAction (written by older code) and assume GitHub.
                let provider_action: ProviderAction = serde_json::from_str(json).or_else(|_| {
                    let action: QueuedAction = serde_json::from_str(json)?;
                    Ok::<_, anyhow::Error>(ProviderAction {
                        provider: Provider::GitHub,
                        action,
                    })
                })?;
                Ok(PendingAction {
                    id: *id,
                    provider_action,
                })
            })
            .collect()
    }

    pub fn delete_action(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM queued_actions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn pending_count(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        match conn.query_row("SELECT COUNT(*) FROM queued_actions", [], |r| {
            r.get::<_, i64>(0)
        }) {
            Ok(n) => n as usize,
            Err(e) => {
                eprintln!("Warning: failed to query pending action count: {}", e);
                0
            }
        }
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::github::{GitHubLabel, GitHubRef, GitHubUser};

    impl Database {
        fn open_in_memory() -> Result<Self> {
            let conn = Connection::open_in_memory()?;
            let db = Database {
                conn: Mutex::new(conn),
            };
            db.init_schema()?;
            Ok(db)
        }
    }

    fn make_pr(number: u64, state: &str) -> GitHubPR {
        GitHubPR {
            number,
            title: format!("PR #{}", number),
            state: state.to_string(),
            html_url: format!("https://github.com/test/repo/pull/{}", number),
            head: GitHubRef {
                ref_name: "feature".to_string(),
                sha: "abc123".to_string(),
            },
            base: GitHubRef {
                ref_name: "master".to_string(),
                sha: "def456".to_string(),
            },
            user: GitHubUser {
                login: "tester".to_string(),
            },
            mergeable: None,
            merged: None,
            labels: vec![],
            body: None,
            commits: None,
            comments: None,
            review_comments: None,
            additions: None,
            deletions: None,
            changed_files: None,
            updated_at: None,
        }
    }

    fn make_details() -> CachedPrDetails {
        let mut statuses = HashMap::new();
        statuses.insert("ci/test".to_string(), "success".to_string());
        CachedPrDetails {
            statuses,
            comments: vec![GitHubComment {
                id: 1,
                user: GitHubUser {
                    login: "reviewer".to_string(),
                },
                body: "LGTM".to_string(),
                created_at: "2024-01-01T00:00:00Z".to_string(),
            }],
            files: vec![GitHubFile {
                filename: "src/main.rs".to_string(),
                status: "modified".to_string(),
                additions: 5,
                deletions: 2,
                changes: 7,
                previous_filename: None,
                patch: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
            }],
            commits: vec![],
            status_urls: HashMap::new(),
        }
    }

    // ── PR list cache ─────────────────────────────────────────────────────────

    #[test]
    fn test_cache_and_load_prs_all() {
        let db = Database::open_in_memory().unwrap();
        let prs = vec![make_pr(1, "open"), make_pr(2, "open"), make_pr(3, "closed")];
        db.cache_prs("", &prs).unwrap();
        let loaded = db.load_prs("", "all").unwrap();
        assert_eq!(loaded.len(), 3);
    }

    #[test]
    fn test_load_prs_filter_open() {
        let db = Database::open_in_memory().unwrap();
        let prs = vec![make_pr(1, "open"), make_pr(2, "open"), make_pr(3, "closed")];
        db.cache_prs("", &prs).unwrap();
        let open = db.load_prs("", "open").unwrap();
        assert_eq!(open.len(), 2);
    }

    #[test]
    fn test_load_prs_filter_closed() {
        let db = Database::open_in_memory().unwrap();
        let prs = vec![make_pr(1, "open"), make_pr(2, "closed")];
        db.cache_prs("", &prs).unwrap();
        let closed = db.load_prs("", "closed").unwrap();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].number, 2);
    }

    #[test]
    fn test_load_prs_preserves_fields() {
        let db = Database::open_in_memory().unwrap();
        let mut pr = make_pr(42, "open");
        pr.title = "Special title".to_string();
        pr.labels = vec![GitHubLabel {
            name: "ack".to_string(),
            color: "00ff00".to_string(),
        }];
        pr.updated_at = Some("2024-06-01T12:00:00Z".to_string());
        db.cache_prs("", &[pr]).unwrap();
        let loaded = db.load_prs("", "open").unwrap();
        assert_eq!(loaded[0].title, "Special title");
        assert_eq!(loaded[0].labels.len(), 1);
        assert_eq!(loaded[0].labels[0].name, "ack");
        assert_eq!(
            loaded[0].updated_at.as_deref(),
            Some("2024-06-01T12:00:00Z")
        );
    }

    #[test]
    fn test_load_prs_empty() {
        let db = Database::open_in_memory().unwrap();
        let prs = db.load_prs("", "open").unwrap();
        assert!(prs.is_empty());
    }

    #[test]
    fn test_cache_prs_replace_on_conflict() {
        let db = Database::open_in_memory().unwrap();
        let mut pr = make_pr(1, "open");
        db.cache_prs("", &[pr.clone()]).unwrap();
        // Update and re-cache; INSERT OR REPLACE should update
        pr.title = "Updated title".to_string();
        db.cache_prs("", &[pr]).unwrap();
        let loaded = db.load_prs("", "all").unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "Updated title");
    }

    // ── Profile isolation ─────────────────────────────────────────────────────

    #[test]
    fn test_profile_isolation_pr_list() {
        let db = Database::open_in_memory().unwrap();
        db.cache_prs("", &[make_pr(42, "open")]).unwrap();
        db.cache_prs("codeberg", &[make_pr(42, "closed")]).unwrap();

        let default = db.load_prs("", "all").unwrap();
        let cb = db.load_prs("codeberg", "all").unwrap();
        assert_eq!(default[0].state, "open");
        assert_eq!(cb[0].state, "closed");
    }

    // ── PR details cache ──────────────────────────────────────────────────────

    #[test]
    fn test_cache_and_load_pr_details() {
        let db = Database::open_in_memory().unwrap();
        let details = make_details();
        db.cache_pr_details("", 42, &details, Some("2024-01-01T00:00:00Z"))
            .unwrap();
        let loaded = db.load_pr_details("", 42).unwrap().unwrap();
        assert_eq!(loaded.statuses.get("ci/test"), Some(&"success".to_string()));
        assert_eq!(loaded.comments.len(), 1);
        assert_eq!(loaded.comments[0].body, "LGTM");
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files[0].filename, "src/main.rs");
        assert_eq!(loaded.files[0].additions, 5);
    }

    #[test]
    fn test_load_pr_details_missing() {
        let db = Database::open_in_memory().unwrap();
        let result = db.load_pr_details("", 999).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_pr_details_updated_at_present() {
        let db = Database::open_in_memory().unwrap();
        let details = make_details();
        db.cache_pr_details("", 42, &details, Some("2024-06-15T10:30:00Z"))
            .unwrap();
        let ts = db.pr_details_updated_at("", 42).unwrap();
        assert_eq!(ts, "2024-06-15T10:30:00Z");
    }

    #[test]
    fn test_pr_details_updated_at_missing() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.pr_details_updated_at("", 42).is_none());
    }

    #[test]
    fn test_pr_details_updated_at_null() {
        let db = Database::open_in_memory().unwrap();
        let details = make_details();
        db.cache_pr_details("", 42, &details, None).unwrap();
        assert!(db.pr_details_updated_at("", 42).is_none());
    }

    #[test]
    fn test_pr_details_profile_isolation() {
        let db = Database::open_in_memory().unwrap();
        let details = make_details();
        db.cache_pr_details("", 42, &details, Some("2024-01-01"))
            .unwrap();
        // "codeberg" profile PR #42 not cached
        assert!(db.load_pr_details("codeberg", 42).unwrap().is_none());
        assert!(db.pr_details_updated_at("codeberg", 42).is_none());
    }

    // ── Queued actions ────────────────────────────────────────────────────────

    #[test]
    fn test_queue_and_pending_actions() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.pending_count(), 0);
        let action = ProviderAction {
            provider: Provider::GitHub,
            action: QueuedAction::Ack {
                pr_number: 42,
                comment: Some("LGTM".to_string()),
            },
        };
        db.queue_action(&action).unwrap();
        assert_eq!(db.pending_count(), 1);
        let pending = db.pending_actions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].provider_action.provider, Provider::GitHub);
        match &pending[0].provider_action.action {
            QueuedAction::Ack { pr_number, comment } => {
                assert_eq!(*pr_number, 42);
                assert_eq!(comment.as_deref(), Some("LGTM"));
            }
            _ => panic!("Expected Ack"),
        }
    }

    #[test]
    fn test_delete_action() {
        let db = Database::open_in_memory().unwrap();
        let action = ProviderAction {
            provider: Provider::GitHub,
            action: QueuedAction::PostComment {
                pr_number: 1,
                body: "hello".to_string(),
            },
        };
        db.queue_action(&action).unwrap();
        let pending = db.pending_actions().unwrap();
        assert_eq!(pending.len(), 1);
        db.delete_action(pending[0].id).unwrap();
        assert_eq!(db.pending_count(), 0);
        assert!(db.pending_actions().unwrap().is_empty());
    }

    #[test]
    fn test_pending_count_multiple() {
        let db = Database::open_in_memory().unwrap();
        for i in 0..5u64 {
            let action = ProviderAction {
                provider: Provider::GitHub,
                action: QueuedAction::AddLabel {
                    pr_number: i,
                    label: "ack".to_string(),
                },
            };
            db.queue_action(&action).unwrap();
        }
        assert_eq!(db.pending_count(), 5);
    }

    #[test]
    fn test_pending_actions_ordering() {
        let db = Database::open_in_memory().unwrap();
        for i in 0..3u64 {
            let action = ProviderAction {
                provider: Provider::GitHub,
                action: QueuedAction::AddLabel {
                    pr_number: i,
                    label: "x".to_string(),
                },
            };
            db.queue_action(&action).unwrap();
        }
        let pending = db.pending_actions().unwrap();
        // Should be in insertion order (AUTOINCREMENT id ascending)
        let numbers: Vec<u64> = pending
            .iter()
            .map(|p| match &p.provider_action.action {
                QueuedAction::AddLabel { pr_number, .. } => *pr_number,
                _ => panic!(),
            })
            .collect();
        assert_eq!(numbers, vec![0, 1, 2]);
    }

    #[test]
    fn test_backward_compat_plain_queued_action() {
        let db = Database::open_in_memory().unwrap();
        // Old format: plain QueuedAction JSON without the provider wrapper
        let old_json = r#"{"type":"Ack","pr_number":99,"comment":null}"#;
        let conn = db.conn.lock().unwrap();
        let now = now_secs();
        conn.execute(
            "INSERT INTO queued_actions (action_json, created_at) VALUES (?1, ?2)",
            rusqlite::params![old_json, now],
        )
        .unwrap();
        drop(conn);

        let pending = db.pending_actions().unwrap();
        assert_eq!(pending.len(), 1);
        // Backward compat: assume GitHub for old entries
        assert_eq!(pending[0].provider_action.provider, Provider::GitHub);
        match &pending[0].provider_action.action {
            QueuedAction::Ack { pr_number, .. } => assert_eq!(*pr_number, 99),
            _ => panic!("Expected Ack"),
        }
    }

    #[test]
    fn test_all_queued_action_variants_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        let actions = vec![
            ProviderAction {
                provider: Provider::GitHub,
                action: QueuedAction::AddLabel {
                    pr_number: 1,
                    label: "ack".to_string(),
                },
            },
            ProviderAction {
                provider: Provider::Forgejo,
                action: QueuedAction::RemoveLabel {
                    pr_number: 2,
                    label: "rejected".to_string(),
                },
            },
            ProviderAction {
                provider: Provider::GitHub,
                action: QueuedAction::PostComment {
                    pr_number: 3,
                    body: "nice".to_string(),
                },
            },
            ProviderAction {
                provider: Provider::GitHub,
                action: QueuedAction::PostReviewComment {
                    pr_number: 4,
                    commit_id: "abc".to_string(),
                    path: "src/lib.rs".to_string(),
                    line: 10,
                    body: "fix this".to_string(),
                },
            },
            ProviderAction {
                provider: Provider::GitHub,
                action: QueuedAction::Ack {
                    pr_number: 5,
                    comment: None,
                },
            },
            ProviderAction {
                provider: Provider::GitHub,
                action: QueuedAction::Reject {
                    pr_number: 6,
                    comment: "not ready".to_string(),
                },
            },
            ProviderAction {
                provider: Provider::GitHub,
                action: QueuedAction::UpdateLabels {
                    pr_number: 7,
                    to_add: vec!["ack".to_string()],
                    to_remove: vec!["rejected".to_string()],
                },
            },
        ];
        for action in &actions {
            db.queue_action(action).unwrap();
        }
        let pending = db.pending_actions().unwrap();
        assert_eq!(pending.len(), actions.len());

        assert_eq!(pending[1].provider_action.provider, Provider::Forgejo);
        match &pending[3].provider_action.action {
            QueuedAction::PostReviewComment { line, path, .. } => {
                assert_eq!(*line, 10);
                assert_eq!(path, "src/lib.rs");
            }
            _ => panic!("Expected PostReviewComment"),
        }
        match &pending[6].provider_action.action {
            QueuedAction::UpdateLabels {
                to_add, to_remove, ..
            } => {
                assert_eq!(to_add, &["ack".to_string()]);
                assert_eq!(to_remove, &["rejected".to_string()]);
            }
            _ => panic!("Expected UpdateLabels"),
        }
    }

    // ── Provider enum ─────────────────────────────────────────────────────────

    #[test]
    fn test_provider_as_str() {
        assert_eq!(Provider::GitHub.as_str(), "github");
        assert_eq!(Provider::Forgejo.as_str(), "forgejo");
    }

    #[test]
    fn test_provider_serde_roundtrip() {
        let pa = ProviderAction {
            provider: Provider::Forgejo,
            action: QueuedAction::Ack {
                pr_number: 1,
                comment: None,
            },
        };
        let json = serde_json::to_string(&pa).unwrap();
        let pa2: ProviderAction = serde_json::from_str(&json).unwrap();
        assert_eq!(pa2.provider, Provider::Forgejo);
    }
}
