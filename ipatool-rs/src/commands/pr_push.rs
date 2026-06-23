use anyhow::{bail, Result};
use regex::Regex;
use std::sync::OnceLock;

use super::Ctx;
use crate::patch::delete_patches;

static BACKPORT_BRANCH_RE: OnceLock<Regex> = OnceLock::new();
fn backport_branch_re() -> &'static Regex {
    BACKPORT_BRANCH_RE.get_or_init(|| {
        Regex::new(r"^ipa-\d+-\d+$")
            .expect("BACKPORT_BRANCH_RE pattern is valid; failure is a compile-time bug")
    })
}

pub fn run(
    ctx: &mut Ctx,
    pr_id: u64,
    reviewer_args: &[String],
    backport_branches: &[String],
    autobackport: bool,
) -> Result<()> {
    let prc = ctx.pr_client_or_err()?.clone();

    let patchdir = ctx.config.patchdir_expanded();
    let pr = prc.get_pr(pr_id)?;
    let labels = prc.pr_label_names(pr_id)?;

    if prc.pr_is_closed(pr_id)? {
        bail!("Pull request is already closed");
    }
    if !labels.iter().any(|l| l == "ack") {
        bail!("Pull request is not ACKed");
    }
    if labels.iter().any(|l| l == "rejected") {
        bail!("Pull request is rejected");
    }
    if labels.iter().any(|l| l == "pushed") {
        bail!("Pull request was already pushed");
    }
    if !pr.mergeable.unwrap_or(true) {
        bail!("Pull request is not mergeable");
    }

    // Check CI statuses
    let statuses = prc.most_recent_statuses(&pr.head.sha)?;
    if statuses
        .values()
        .any(|j| j.state == "error" || j.state == "failure")
    {
        bail!("Pull request failed CI test(s)");
    }
    if statuses.values().any(|j| j.state == "pending") {
        bail!("CI has not completed testing the pull request yet");
    }

    // Download patches from PR
    super::backport::download_pr_patches(&prc, &pr, &patchdir)?;

    // Set target branch from PR base
    let base_branch = pr.base.ref_name.clone();

    // Run the push command
    let push_result = super::push::run(
        ctx,
        &[],
        &[base_branch],
        reviewer_args,
        autobackport,
        backport_branches,
        &[],
    );

    // Post-push actions
    let pushed = ctx.push_info.as_ref().map(|i| i.pushed).unwrap_or(false);

    if !ctx.dry_run && pushed {
        let push_info = ctx.push_info.clone().unwrap_or_default();

        println!("Adding label 'pushed'");
        if let Err(e) = prc.add_labels(pr_id, &["pushed"]) {
            eprintln!(
                "Warning: failed to add 'pushed' label to PR #{} ({}): {}. \
                 Please add the 'pushed' label manually.",
                pr_id, pr.html_url, e
            );
        }

        if let Err(e) = prc.create_comment(pr_id, &push_info.pagure_comment) {
            eprintln!(
                "Warning: failed to create push comment on PR #{} ({}): {}. \
                 Please post the push comment manually.",
                pr_id, pr.html_url, e
            );
        }

        println!("Closing pull request {}", pr_id);
        if let Err(e) = prc.close_pr(pr_id) {
            eprintln!(
                "Warning: failed to close PR #{} ({}): {}. \
                 Please close the pull request manually.",
                pr_id, pr.html_url, e
            );
        }

        // Handle backports
        let mut bp_branches: Vec<String> = backport_branches.to_vec();
        if autobackport {
            for label in &labels {
                if backport_branch_re().is_match(label) && !bp_branches.contains(label) {
                    bp_branches.push(label.clone());
                }
            }
        }

        if !bp_branches.is_empty() {
            super::backport::run_backport(ctx, &bp_branches, &prc, &pr)?;
        }
    }

    delete_patches(&patchdir);

    push_result
}
