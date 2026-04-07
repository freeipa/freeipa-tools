use anyhow::{bail, Result};

use super::Ctx;
use crate::output::prompt;
use crate::patch::collect_patches;

pub fn run(
    ctx: &Ctx,
    force: bool,
    do_am: bool,
    ticket_args: &[u64],
    patch_paths: &[String],
) -> Result<()> {
    if !ctx.has_tracker() {
        bail!("Cannot work without an issue tracker (--no-pagure/--no-forgejo)");
    }

    let patchdir = ctx.config.patchdir_expanded();
    let ticket_url = ctx.config.ticket_url.clone();

    let mut ticket_numbers: Vec<u64> = ticket_args.to_vec();

    let patches = if !patch_paths.is_empty() || ticket_numbers.is_empty() {
        if ticket_numbers.is_empty() {
            println!("\x1b[33mUsing patches from {}\x1b[0m", patchdir.display());
        }
        collect_patches(patch_paths, &patchdir, &ticket_url)?
    } else {
        vec![]
    };

    for patch in &patches {
        ticket_numbers.extend(&patch.ticket_numbers);
    }
    ticket_numbers.sort_unstable();
    ticket_numbers.dedup();

    if ctx.verbosity > 0 {
        println!("Tickets selected: {:?}", ticket_numbers);
    }

    let tickets: Vec<_> = ticket_numbers
        .iter()
        .filter_map(|&n| ctx.make_ticket(n))
        .collect();

    if tickets.is_empty() {
        bail!("No tickets selected");
    }

    let mut existing_reviewers = Vec::new();
    for ticket in &tickets {
        ctx.out.print_blue(&format!("Ticket #{}", ticket.number()));
        let summary = match ticket {
            super::Ticket::Pagure(t) => t.data().map(|d| d.content.clone()).unwrap_or_default(),
            super::Ticket::Forgejo(t) => t
                .data()
                .map(|d| d.body.clone().unwrap_or_default())
                .unwrap_or_default(),
        };
        println!("- summary: {}", summary);
        let reviewer = ticket.reviewer().unwrap_or(None);
        if let Some(ref r) = reviewer {
            println!("- reviewer: \x1b[34m{}\x1b[0m", r);
            existing_reviewers.push(r.clone());
        } else {
            println!("- reviewer: none");
        }
    }

    if !existing_reviewers.is_empty() {
        if force {
            ctx.out.print_yellow("Existing reviewer(s) found");
        } else {
            bail!("Existing reviewer(s) found; won't overwrite without --force");
        }
    }

    if ctx.dry_run {
        bail!("Exiting, --dry-run specified");
    }

    loop {
        let response = prompt("Start review on these tickets? [y/n] ");
        match response.to_lowercase().as_str() {
            "n" => return Ok(()),
            "y" => break,
            _ => {}
        }
    }

    if do_am {
        println!("Applying patches to worktree...");
        am_patches(ctx, &patches)?;
    }

    bail!("Setting reviewer via API is not yet implemented; update tickets manually");
}

pub fn am_patches(ctx: &Ctx, patches: &[crate::patch::Patch]) -> Result<()> {
    for patch in patches {
        println!("Applying patch: {}", patch.display_name());
        let content = patch.content();
        let am_command: Vec<&str> = ctx.config.am_command.iter().map(|s| s.as_str()).collect();
        if am_command.is_empty() {
            bail!("am-command is not configured");
        }
        let result = crate::git::run_process(
            &am_command,
            &ctx.git_env,
            Some(&content),
            true,
            Some(120),
            ctx.verbosity.max(2),
        )?;
        if result.returncode != 0 {
            bail!("am-command failed for patch: {}", patch.display_name());
        }
    }
    Ok(())
}
