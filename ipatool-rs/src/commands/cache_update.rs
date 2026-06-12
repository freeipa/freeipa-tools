use anyhow::{bail, Result};
use std::io::Write;

use super::Ctx;
use crate::api::types::sorted_commits;
use crate::db::CachedPrDetails;
use std::collections::HashMap;

pub fn run(ctx: &Ctx, state: &str) -> Result<()> {
    let prc = ctx.pr_client_or_err()?;
    let Some(db) = &ctx.db else {
        bail!("No local database configured (db-path missing from config)");
    };

    let color = ctx.out.color.enabled();

    ctx.out
        .print_cyan(&format!("Fetching {} pull requests…", state));

    let prs = prc.list_prs(state)?;
    let total = prs.len();
    db.cache_prs(&ctx.profile, &prs)?;
    ctx.out
        .print_green(&format!("Cached {} pull request(s).", total));

    // Fetch and cache supplementary details only for PRs that changed since the
    // details were last stored.  A PR is considered unchanged when its
    // `updated_at` timestamp matches the one recorded during the previous detail
    // fetch.  PRs with no cached details are always fetched.
    let mut fetched = 0usize;
    let mut skipped = 0usize;

    for (i, pr) in prs.iter().enumerate() {
        let cached_updated_at = db.pr_details_updated_at(&ctx.profile, pr.number);
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
                pr.number,
                i + 1,
                total
            )
        } else {
            format!(
                "\rFetching details for PR #{} ({}/{})…\x1b[K",
                pr.number,
                i + 1,
                total
            )
        };
        eprint!("{}", line);
        let _ = std::io::stderr().flush();

        // most_recent_statuses() returns HashMap<String, CiJobStatus>.
        let statuses = prc.most_recent_statuses(&pr.head.sha).unwrap_or_else(|e| {
            eprint!("\r\x1b[K");
            eprintln!(
                "Warning: failed to fetch CI statuses for PR #{}: {}",
                pr.number, e
            );
            Default::default()
        });
        let comments = prc.get_all_issue_comments(pr.number).unwrap_or_else(|e| {
            eprint!("\r\x1b[K");
            eprintln!(
                "Warning: failed to fetch comments for PR #{}: {}",
                pr.number, e
            );
            vec![]
        });
        let files = prc.get_pr_files(pr.number).unwrap_or_else(|e| {
            eprint!("\r\x1b[K");
            eprintln!(
                "Warning: failed to fetch files for PR #{}: {}",
                pr.number, e
            );
            vec![]
        });
        let commits = match prc.get_pr_commits(pr.number) {
            Ok(raw) => sorted_commits(raw).unwrap_or_else(|e| {
                eprint!("\r\x1b[K");
                eprintln!(
                    "Warning: failed to sort commits for PR #{}: {}",
                    pr.number, e
                );
                vec![]
            }),
            Err(e) => {
                eprint!("\r\x1b[K");
                eprintln!(
                    "Warning: failed to fetch commits for PR #{}: {}",
                    pr.number, e
                );
                vec![]
            }
        };

        // Split CiJobStatus into separate state and URL maps for storage.
        let cached_states: HashMap<String, String> = statuses
            .iter()
            .map(|(ctx, job)| (ctx.clone(), job.state.clone()))
            .collect();
        let cached_urls: HashMap<String, String> = statuses
            .iter()
            .filter_map(|(ctx, job)| job.url.as_ref().map(|u| (ctx.clone(), u.clone())))
            .collect();

        let cached = CachedPrDetails {
            statuses: cached_states,
            comments,
            files,
            commits,
            status_urls: cached_urls,
        };
        match db.cache_pr_details(&ctx.profile, pr.number, &cached, pr_updated_at) {
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
