use anyhow::{bail, Result};
use std::io::Write;

use super::Ctx;
use crate::api::github::sorted_commits;
use crate::db::{CachedPrDetails, Provider};

pub fn run(ctx: &Ctx, state: &str) -> Result<()> {
    let Some(gh) = &ctx.github else {
        bail!("GitHub is not configured (gh-token / gh-repo missing)");
    };
    let Some(db) = &ctx.db else {
        bail!("No local database configured (db-path missing from config)");
    };

    let color = ctx.out.color.enabled();

    ctx.out.print_cyan(&format!("Fetching {} pull requests from GitHub…", state));

    let prs = gh.list_prs(state)?;
    let total = prs.len();
    db.cache_prs(Provider::GitHub, &prs)?;
    ctx.out.print_green(&format!("Cached {} pull request(s).", total));

    // Fetch and cache supplementary details only for PRs that changed since the
    // details were last stored.  A PR is considered unchanged when its
    // `updated_at` timestamp matches the one recorded during the previous detail
    // fetch.  PRs with no cached details are always fetched.
    let mut fetched = 0usize;
    let mut skipped = 0usize;

    for (i, pr) in prs.iter().enumerate() {
        let cached_updated_at = db.pr_details_updated_at(Provider::GitHub, pr.number);
        let pr_updated_at = pr.updated_at.as_deref();

        if let (Some(cached), Some(current)) = (cached_updated_at.as_deref(), pr_updated_at) {
            if cached == current {
                skipped += 1;
                continue;
            }
        }

        let line = if color {
            format!(
                "\r\x1b[36mFetching details for PR #{} ({}/{})…\x1b[0m\x1b[K",
                pr.number, i + 1, total
            )
        } else {
            format!(
                "\rFetching details for PR #{} ({}/{})…\x1b[K",
                pr.number, i + 1, total
            )
        };
        eprint!("{}", line);
        let _ = std::io::stderr().flush();

        let statuses = gh.most_recent_statuses(&pr.head.sha).unwrap_or_default();
        let comments = gh.get_all_issue_comments(pr.number).unwrap_or_default();
        let files    = gh.get_pr_files(pr.number).unwrap_or_default();
        let commits  = gh.get_pr_commits(pr.number)
            .ok()
            .and_then(|c| sorted_commits(c).ok())
            .unwrap_or_default();

        let cached = CachedPrDetails { statuses, comments, files, commits };
        match db.cache_pr_details(Provider::GitHub, pr.number, &cached, pr_updated_at) {
            Ok(()) => fetched += 1,
            Err(e) => {
                eprint!("\r\x1b[K");
                let _ = std::io::stderr().flush();
                ctx.out.print_yellow(&format!(
                    "Warning: failed to cache details for PR #{}: {}",
                    pr.number, e
                ));
            }
        }
    }

    // Clear the progress line.
    eprint!("\r\x1b[K");
    let _ = std::io::stderr().flush();

    ctx.out.print_green(&format!(
        "Details: {} updated, {} unchanged (skipped).",
        fetched, skipped
    ));

    Ok(())
}
