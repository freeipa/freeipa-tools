use anyhow::{bail, Result};
use regex::Regex;
use std::collections::{HashMap, HashSet};

use super::{
    apply_patches_to_branch, close_issue, get_reviewers, git_cleanup, milestone_branches,
    update_issue, update_jira_issues, Ctx, PushInfo, TicketOps,
};
use crate::output::prompt;
use crate::patch::{collect_patches, Patch};

pub fn run(
    ctx: &mut Ctx,
    patch_paths: &[String],
    branch_args: &[String],
    reviewer_args: &[String],
    autobackport: bool,
    backport_branches: &[String],
) -> Result<()> {
    let patchdir = ctx.config.patchdir_expanded();
    let ticket_url = &ctx.config.ticket_url;
    let legacy_ticket_url = &ctx.config.legacy_ticket_url;
    let mut patches = collect_patches(patch_paths, &patchdir, ticket_url, legacy_ticket_url)?;

    // Rewrite legacy ticket URLs in commit messages before git-am writes them into history.
    if ctx.config.rewrite_ticket_urls && !legacy_ticket_url.is_empty() && !ticket_url.is_empty() {
        for patch in &mut patches {
            patch.rewrite_urls(legacy_ticket_url, ticket_url);
        }
    }

    if patches.is_empty() {
        bail!("No patches to push");
    }

    // Change to clean repo
    let repo_path = ctx.config.clean_repo_path_expanded();
    std::env::set_current_dir(&repo_path)
        .map_err(|e| anyhow::anyhow!("Cannot cd to {}: {}", repo_path.display(), e))?;

    crate::git::ensure_clean(&ctx.git_env, ctx.verbosity)?;

    // Collect ticket numbers from patches, then apply issue_number_map so that
    // legacy pagure numbers are translated to the corresponding Codeberg numbers
    // when the migration did not preserve the original numbering.
    let mut ticket_numbers: HashSet<u64> = HashSet::new();
    for patch in &patches {
        for &n in &patch.ticket_numbers {
            let mapped = ctx.config.issue_number_map.get(&n).copied().unwrap_or(n);
            ticket_numbers.insert(mapped);
        }
    }

    // Make ticket objects
    let tickets: Vec<_> = if ctx.has_tracker() {
        ticket_numbers
            .iter()
            .filter_map(|&n| ctx.make_ticket(n))
            .collect()
    } else {
        vec![]
    };

    // Get reviewers
    let reviewers = get_reviewers(ctx, reviewer_args, &tickets)?;
    if !reviewers.is_empty() {
        for reviewer in &reviewers {
            println!("Reviewer: {}", reviewer);
        }
        for patch in &mut patches {
            for reviewer in &reviewers {
                patch.add_reviewer(reviewer);
            }
        }
    } else {
        println!("No reviewer");
    }

    // Determine target branches
    let branches: Vec<String> = if !branch_args.is_empty() {
        branch_args.to_vec()
    } else {
        if tickets.is_empty() {
            if ctx.has_tracker() {
                bail!("No branches specified and no tickets found");
            } else {
                bail!("No branches specified and no issue tracker configured");
            }
        }
        // Collect milestones from tickets
        let mut milestones = HashSet::new();
        for ticket in &tickets {
            match ticket.milestone() {
                Ok(Some(m)) => {
                    milestones.insert(m);
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!(
                        "Warning: could not retrieve milestone from ticket: {}",
                        e
                    );
                }
            }
        }
        if milestones.is_empty() {
            bail!("No milestones found in tickets");
        }
        if milestones.len() > 1 {
            bail!(
                "Tickets belong to disparate milestones; fix them or specify branches explicitly"
            );
        }
        let milestone = milestones.into_iter().next().unwrap();
        match milestone_branches(&milestone) {
            Some(b) => b,
            None => bail!(
                "No branches correspond to milestone '{}'. Update MILESTONES in ipatool.",
                milestone
            ),
        }
    };

    println!(
        "Will apply {} patches to: {}",
        patches.len(),
        branches.join(", ")
    );

    let remote = ctx.config.remote.clone();
    ctx.verify_remote_url()?;

    if !ctx.no_fetch {
        println!("Fetching...");
        crate::git::fetch(&remote, &ctx.git_env, ctx.verbosity)?;
    }

    let old_branch = crate::git::current_branch(&ctx.git_env, ctx.verbosity)?;

    let result = (|| -> Result<bool> {
        // Apply patches to all branches
        let mut sha1s: HashMap<String, String> = HashMap::new();
        for branch in &branches {
            let sha = apply_patches_to_branch(ctx, &patches, branch, true)?;
            sha1s.insert(branch.clone(), sha);
        }

        // Dry-run push
        let push_args: Vec<String> = branches
            .iter()
            .map(|b| format!("{}:{}", sha1s[b], b))
            .collect();

        println!("Trying push...");
        crate::git::push_dry_run(&remote, &push_args, &ctx.git_env, ctx.verbosity)?;

        println!("Generating info...");
        let push_info = build_push_info(ctx, &patches, &sha1s, &ticket_numbers, &tickets)?;
        ctx.push_info = Some(push_info);

        if ctx.dry_run {
            println!("Exiting, --dry-run specified");
            return Ok(false);
        }

        // Interactive push confirmation
        println!("Starting the push...");
        loop {
            println!("(k will start `gitk`)");
            let branches_repr = branches.join(", ");
            let response = prompt(&format!("Push to {}? [y/n/k] ", branches_repr));
            match response.to_lowercase().as_str() {
                "n" => return Ok(false),
                "k" => {
                    let sha_vals: Vec<String> = branches.iter().map(|b| sha1s[b].clone()).collect();
                    crate::git::gitk(&branches, &sha_vals, &ctx.git_env)?;
                }
                "y" => {
                    println!("Pushing");
                    crate::git::push(&remote, &push_args, &ctx.git_env, ctx.verbosity)?;
                    return Ok(true);
                }
                _ => {}
            }
        }
    })();

    git_cleanup(ctx, &old_branch);

    let pushed = match result {
        Ok(p) => p,
        Err(e) => {
            bail!("Push failed: {}", e);
        }
    };

    if let Some(push_info) = ctx.push_info.as_mut() {
        push_info.pushed = pushed;
    }

    if pushed {
        let has_backport = autobackport || !backport_branches.is_empty();
        for ticket in &tickets {
            update_issue(ctx, ticket);
            close_issue(ctx, ticket, has_backport);
        }
        update_jira_issues(ctx);
    }

    Ok(())
}

fn build_push_info(
    ctx: &Ctx,
    patches: &[Patch],
    sha1s: &HashMap<String, String>,
    ticket_numbers: &HashSet<u64>,
    tickets: &[super::Ticket],
) -> Result<PushInfo> {
    let remote = &ctx.config.remote;
    let mut branches: Vec<&String> = sha1s.keys().collect();
    branches.sort();

    let mut pagure_log: Vec<String> = Vec::new();
    let mut bugzilla_log = vec!["Fixed upstream".to_string()];

    for branch in &branches {
        let sha = &sha1s[*branch];
        pagure_log.push(format!("{}:\n", branch));
        bugzilla_log.push(format!("{}:", branch));

        let log = crate::git::log_graph_oneline(remote, branch, sha, &ctx.git_env, ctx.verbosity)?;
        let lines: Vec<String> = log
            .lines()
            .map(|l| l.trim_end().to_string())
            .rev()
            .collect();
        pagure_log.extend(lines);
        pagure_log.push("\n".to_string());

        let hashes = crate::git::log_hashes(remote, branch, sha, &ctx.git_env, ctx.verbosity)?;
        for hash in hashes.iter().rev() {
            bugzilla_log.push(format!("{}{}", ctx.config.commit_url, hash));
        }
    }

    // Extract jira URLs from tickets
    let jira_ticket_url = &ctx.config.jira_ticket_url;
    let bugzilla_bug_url = &ctx.config.bugzilla_bug_url;
    let mut jira_urls: Vec<String> = Vec::new();
    let mut bugzilla_urls: Vec<String> = Vec::new();

    if !jira_ticket_url.is_empty() {
        let jira_re = Regex::new(&format!(r"({}[\d]+)", regex::escape(jira_ticket_url))).ok();
        let bz_re = if bugzilla_bug_url.is_empty() {
            None
        } else {
            Some(
                Regex::new(&format!(r"({}[\d]+)", regex::escape(bugzilla_bug_url))).expect(
                    "bugzilla_bug_url regex: escaped URL should always produce valid regex",
                ),
            )
        };

        for ticket in tickets {
            if let Ok(Some(rhbz)) = ticket.rhbz() {
                if let Some(re) = &bz_re {
                    for cap in re.captures_iter(&rhbz) {
                        if let Some(m) = cap.get(1) {
                            bugzilla_urls.push(m.as_str().to_string());
                        }
                    }
                }
                if let Some(re) = &jira_re {
                    for cap in re.captures_iter(&rhbz) {
                        if let Some(m) = cap.get(1) {
                            jira_urls.push(m.as_str().to_string());
                        }
                    }
                }
            }
        }
    }

    // Display info
    for branch in &branches {
        let sha = &sha1s[*branch];
        ctx.out.section(&format!("Diffstat for {}", branch));
        crate::git::diff_stat(remote, branch, sha, &ctx.color, &ctx.git_env, ctx.verbosity)?;
        ctx.out.section(&format!("Log for {}", branch));
        crate::git::log_full(remote, branch, sha, &ctx.color, &ctx.git_env, ctx.verbosity)?;
    }

    ctx.out.section("Patches pushed");
    for patch in patches {
        println!("{}", patch.display_name());
    }

    ctx.out.section("Mail summary");
    if branches.len() == 1 {
        print!("Pushed to ");
    } else {
        println!("Pushed to:");
    }
    for branch in &branches {
        println!("{}: {}", branch, sha1s[*branch]);
    }

    let pagure_msg = pagure_log.join("\n");
    ctx.out.section("Ticket comment");
    println!("{}", pagure_msg);

    let bugzilla_msg = bugzilla_log.join("\n");
    ctx.out.section("Bugzilla/JIRA comment");
    println!("{}", bugzilla_msg);

    if !ticket_numbers.is_empty() {
        ctx.out.section("Tickets fixed");
        let mut sorted: Vec<u64> = ticket_numbers.iter().cloned().collect();
        sorted.sort_unstable();
        for n in sorted {
            println!("{}{}", ctx.config.ticket_url, n);
        }
    }

    if !bugzilla_urls.is_empty() {
        ctx.out.section("Bugzillas fixed");
        println!("{}", bugzilla_urls.join("\n"));
    }

    if !jira_urls.is_empty() {
        ctx.out.section("Jira tickets fixed");
        println!("{}", jira_urls.join("\n"));
    }

    ctx.out.section("Ready to push");

    Ok(PushInfo {
        pushed: false,
        pagure_comment: pagure_msg,
        bugzilla_comment: bugzilla_msg,
        jira_urls,
    })
}
