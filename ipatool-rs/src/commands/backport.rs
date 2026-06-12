use anyhow::{bail, Result};
use std::sync::Arc;

use super::pr_client::PrClient;
use super::Ctx;
use crate::api::github::{sorted_commits, GitHubPR};
use crate::patch::{delete_patches, patch_filename};

pub fn run_backport_cmd(ctx: &mut Ctx, pr_id: u64, branches: &[String]) -> Result<()> {
    let prc = ctx.pr_client_or_err()?.clone();

    let patchdir = ctx.config.patchdir_expanded();
    let pr = prc.get_pr(pr_id)?;
    let labels = prc.pr_label_names(pr_id)?;

    if !labels.contains(&"ack".to_string()) {
        bail!("Pull request is not ACKed");
    }
    if labels.contains(&"rejected".to_string()) {
        bail!("Pull request is rejected");
    }
    if !labels.contains(&"pushed".to_string()) && !pr.mergeable.unwrap_or(true) {
        bail!("Pull request is not mergeable");
    }

    // Check CI
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

    // Download commits as patches
    download_pr_patches(&prc, &pr, &patchdir)?;

    let result = run_backport(ctx, branches, &prc, &pr);

    delete_patches(&patchdir);
    result
}

pub fn run_backport(
    ctx: &mut Ctx,
    backport_branches: &[String],
    prc: &Arc<PrClient>,
    pr: &GitHubPR,
) -> Result<()> {
    let fork_remote = ctx.config.gh_fork_remote.clone();
    if fork_remote.is_empty() {
        bail!("gh-fork-remote is not configured");
    }

    let patchdir = ctx.config.patchdir_expanded();
    let ticket_url = &ctx.config.ticket_url;
    let legacy_ticket_url = &ctx.config.legacy_ticket_url;
    let mut patches = crate::patch::collect_patches(&[], &patchdir, ticket_url, legacy_ticket_url)?;

    // Rewrite legacy ticket URLs in commit messages before git-am writes them into history.
    if ctx.config.rewrite_ticket_urls && !legacy_ticket_url.is_empty() && !ticket_url.is_empty() {
        for patch in &mut patches {
            patch.rewrite_urls(legacy_ticket_url, ticket_url);
        }
    }

    let repo_path = ctx.config.clean_repo_path_expanded();
    std::env::set_current_dir(&repo_path)
        .map_err(|e| anyhow::anyhow!("Cannot cd to {}: {}", repo_path.display(), e))?;

    let user_login = prc.get_authenticated_user_login()?;
    let old_branch = crate::git::current_branch(&ctx.git_env, ctx.verbosity)?;

    for bb in backport_branches {
        let result = (|| -> Result<()> {
            // Checkout remote branch
            let checkout_result = crate::git::run_process(
                &["git", "checkout", &format!("{}/{}", ctx.config.remote, bb)],
                &ctx.git_env,
                None,
                false,
                None,
                ctx.verbosity,
            )?;
            if checkout_result.returncode != 0 {
                println!(
                    "\x1b[31mFailed to checkout {}/{}. Manual backport is needed. {}\x1b[0m",
                    ctx.config.remote, bb, checkout_result.stderr
                );
                return Ok(());
            }

            // Apply patches
            let sha = match super::apply_patches_to_branch(ctx, &patches, bb, false) {
                Ok(sha) => sha,
                Err(e) => {
                    println!(
                        "\x1b[31mFailed to apply patches onto {}/{}. Manual backport is needed.\x1b[0m",
                        ctx.config.remote, bb
                    );
                    println!("\x1b[31m{}\x1b[0m", e);
                    return Ok(());
                }
            };

            println!("Applied patches on {}/{}", ctx.config.remote, bb);

            let backport_name = format!("backport_pr{}_{}", pr.number, bb);
            crate::git::push_to_fork(
                &fork_remote,
                &sha,
                &backport_name,
                &ctx.git_env,
                ctx.verbosity,
            )
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to push {} to {}/{}: {}",
                    sha,
                    fork_remote,
                    backport_name,
                    e
                )
            })?;

            println!("Pushed {} to {}/{}", sha, fork_remote, backport_name);

            let backport_pr = prc.create_pr(
                &format!("[Backport][{}] {}", bb, pr.title),
                bb,
                &format!("{}:{}", user_login, backport_name),
                &format!(
                    "This PR was opened automatically because PR #{} was pushed to {} \
                     and backport to {} is required.",
                    pr.number, pr.base.ref_name, bb
                ),
            )?;

            prc.add_labels(backport_pr.number, &["ack"])?;
            prc.create_comment(
                backport_pr.number,
                &format!(
                    "PR was ACKed automatically because this is a backport of PR #{}. \
                     Wait for CI to finish before pushing. In case of questions or problems \
                     contact @{} who is the author of the original PR.",
                    pr.number, pr.user.login
                ),
            )?;

            println!(
                "\x1b[32mCreated and auto-ACKed PR {} against branch {}: {}\x1b[0m",
                backport_pr.number, bb, backport_pr.html_url
            );

            Ok(())
        })();

        super::git_cleanup(ctx, &old_branch);

        if let Err(e) = result {
            println!("\x1b[31mBackport to {} failed: {}\x1b[0m", bb, e);
        }
    }

    Ok(())
}

pub fn download_pr_patches(
    prc: &Arc<PrClient>,
    pr: &GitHubPR,
    patchdir: &std::path::Path,
) -> Result<()> {
    let commits_raw = prc.get_pr_commits(pr.number)?;
    let commits = sorted_commits(commits_raw)?;

    for (num, commit) in commits.iter().enumerate() {
        let filename = patch_filename(&commit.commit.message, num + 1);
        let path = patchdir.join(&filename);
        let patch_bytes = prc.get_commit_patch(&commit.sha)?;
        std::fs::write(&path, &patch_bytes)
            .map_err(|e| anyhow::anyhow!("Cannot write patch {}: {}", path.display(), e))?;
    }

    Ok(())
}
