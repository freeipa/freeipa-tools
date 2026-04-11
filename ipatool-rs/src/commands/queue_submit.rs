use anyhow::{bail, Result};

use super::Ctx;
use crate::db::QueuedAction;

fn describe(action: &QueuedAction) -> String {
    match action {
        QueuedAction::Ack { pr_number, comment } => {
            if comment.is_some() {
                format!("ACK PR #{} (with comment)", pr_number)
            } else {
                format!("ACK PR #{}", pr_number)
            }
        }
        QueuedAction::Reject { pr_number, .. } => format!("Reject PR #{}", pr_number),
        QueuedAction::AddLabel { pr_number, label } => {
            format!("Add label '{}' to PR #{}", label, pr_number)
        }
        QueuedAction::RemoveLabel { pr_number, label } => {
            format!("Remove label '{}' from PR #{}", label, pr_number)
        }
        QueuedAction::PostComment { pr_number, .. } => {
            format!("Post comment on PR #{}", pr_number)
        }
        QueuedAction::PostReviewComment {
            pr_number,
            path,
            line,
            ..
        } => {
            format!(
                "Post review comment on PR #{} ({}:{})",
                pr_number, path, line
            )
        }
        QueuedAction::UpdateLabels {
            pr_number,
            to_add,
            to_remove,
        } => {
            format!(
                "Update labels on PR #{} (+[{}] -[{}])",
                pr_number,
                to_add.join(", "),
                to_remove.join(", ")
            )
        }
    }
}

/// List queued actions without submitting them.
pub fn run_list(ctx: &Ctx) -> Result<()> {
    let Some(db) = &ctx.db else {
        bail!("No local database configured (db-path missing from config)");
    };

    let pending = db.pending_actions()?;
    if pending.is_empty() {
        println!("No queued actions.");
        return Ok(());
    }

    println!("{} queued action(s):", pending.len());
    for p in &pending {
        println!(
            "  [{}] ({}) {}",
            p.id,
            p.provider_action.provider.as_str(),
            describe(&p.provider_action.action)
        );
    }
    Ok(())
}

/// Submit all queued actions to the upstream forge.
pub fn run_submit(ctx: &Ctx) -> Result<()> {
    let Some(db) = &ctx.db else {
        bail!("No local database configured (db-path missing from config)");
    };
    let prc = ctx.pr_client_or_err()?;

    let pending = db.pending_actions()?;
    if pending.is_empty() {
        println!("No queued actions to submit.");
        return Ok(());
    }

    ctx.out
        .print_cyan(&format!("Submitting {} queued action(s)…", pending.len()));

    let mut submitted = 0usize;
    let mut failed = 0usize;

    for p in &pending {
        let desc = describe(&p.provider_action.action);
        print!("  [{}] {} … ", p.id, desc);

        let result: anyhow::Result<()> = match &p.provider_action.action {
            QueuedAction::Ack { pr_number, comment } => {
                super::pr_ack::run_api(prc, *pr_number, comment.as_deref())
            }
            QueuedAction::Reject { pr_number, comment } => {
                super::pr_reject::run_api(prc, *pr_number, comment)
            }
            QueuedAction::AddLabel { pr_number, label } => {
                prc.add_labels(*pr_number, &[label.as_str()])
            }
            QueuedAction::RemoveLabel { pr_number, label } => prc.remove_label(*pr_number, label),
            QueuedAction::PostComment { pr_number, body } => prc.create_comment(*pr_number, body),
            QueuedAction::PostReviewComment {
                pr_number,
                commit_id,
                path,
                line,
                body,
            } => prc.create_review_comment(*pr_number, commit_id, path, *line, body),
            QueuedAction::UpdateLabels {
                pr_number,
                to_add,
                to_remove,
            } => {
                let add_refs: Vec<&str> = to_add.iter().map(|s| s.as_str()).collect();
                let mut r = if !add_refs.is_empty() {
                    prc.add_labels(*pr_number, &add_refs)
                } else {
                    Ok(())
                };
                if r.is_ok() {
                    for label in to_remove {
                        r = prc.remove_label(*pr_number, label);
                        if r.is_err() {
                            break;
                        }
                    }
                }
                r
            }
        };

        match result {
            Ok(()) => {
                println!("OK");
                db.delete_action(p.id)?;
                submitted += 1;
            }
            Err(e) => {
                println!("FAILED: {:#}", e);
                failed += 1;
            }
        }
    }

    if failed == 0 {
        ctx.out.print_green(&format!(
            "All {} action(s) submitted successfully.",
            submitted
        ));
    } else {
        ctx.out.print_yellow(&format!(
            "{} submitted, {} failed (still in queue).",
            submitted, failed
        ));
    }

    Ok(())
}
