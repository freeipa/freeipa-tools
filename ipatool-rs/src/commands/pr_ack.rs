use anyhow::{bail, Result};
use std::sync::Arc;

use super::pr_client::PrClient;
use super::Ctx;
use crate::db::{ProviderAction, QueuedAction};

pub fn run(ctx: &Ctx, pr_id: u64, comment: Option<&str>) -> Result<()> {
    if ctx.offline {
        let Some(db) = &ctx.db else {
            bail!("--offline requires a configured database (db-path in config)");
        };
        let prc = ctx.pr_client_or_err()?;
        let action = ProviderAction {
            provider: prc.provider(),
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
    let prc = ctx.pr_client_or_err()?;
    run_api(prc, pr_id, comment)
}

/// Core ACK logic, usable without a full Ctx (e.g. from the TUI).
pub fn run_api(prc: &Arc<PrClient>, pr_id: u64, comment: Option<&str>) -> Result<()> {
    let labels = prc.pr_label_names(pr_id)?;

    if prc.pr_is_closed(pr_id)? {
        bail!("Pull request was already closed");
    }

    if labels.contains(&"rejected".to_string()) {
        bail!("Pull request was rejected");
    }

    prc.add_labels(pr_id, &["ack"])?;

    if let Some(text) = comment {
        if !text.is_empty() {
            prc.create_comment(pr_id, text)?;
        }
    }

    Ok(())
}
