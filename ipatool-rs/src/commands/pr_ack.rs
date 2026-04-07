use anyhow::{bail, Result};
use std::sync::Arc;

use super::Ctx;
use crate::api::github::GitHubClient;
use crate::db::{Provider, ProviderAction, QueuedAction};

pub fn run(ctx: &Ctx, pr_id: u64, comment: Option<&str>) -> Result<()> {
    if ctx.offline {
        let Some(db) = &ctx.db else {
            bail!("--offline requires a configured database (db-path in config)");
        };
        let action = ProviderAction {
            provider: Provider::GitHub,
            action: QueuedAction::Ack {
                pr_number: pr_id,
                comment: comment.map(|s| s.to_string()),
            },
        };
        db.queue_action(&action)?;
        ctx.out.print_cyan(&format!(
            "Queued: ACK PR #{} (submit with `ipatool queue-submit`)",
            pr_id
        ));
        return Ok(());
    }
    let Some(gh) = &ctx.github else {
        bail!("GitHub is not configured (gh-token / gh-repo missing)");
    };
    run_api(gh, pr_id, comment)
}

/// Core ACK logic, usable without a full Ctx (e.g. from the TUI).
pub fn run_api(gh: &Arc<GitHubClient>, pr_id: u64, comment: Option<&str>) -> Result<()> {
    let issue = gh.get_issue(pr_id)?;

    if issue.is_closed() {
        bail!("Pull request was already closed");
    }

    let labels = issue.label_names();
    if labels.contains(&"rejected".to_string()) {
        bail!("Pull request was rejected");
    }

    gh.add_labels(pr_id, &["ack"])?;

    if let Some(text) = comment {
        if !text.is_empty() {
            gh.create_comment(pr_id, text)?;
        }
    }

    Ok(())
}
