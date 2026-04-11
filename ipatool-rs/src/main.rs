mod api;
mod ci;
mod commands;
mod config;
mod db;
mod git;
mod md_render;
mod output;
mod patch;
mod tui_keys;
mod tui_style;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::Ctx;
use config::Config;
use output::{ColorMode, Output};
use std::collections::HashMap;
use std::sync::Arc;

const SAMPLE_CONFIG: &str = r#"
# Local "clean" repository
clean-repo-path: ~/dev/freeipa-clean
remote: origin

# Default directory where patches to push are stored
patchdir: ~/patches/to-apply

# URLs to use in reports & messages
ticket-url: https://pagure.io/freeipa/issue/
commit-url: https://pagure.io/freeipa/c/
bugzilla-bug-url: https://bugzilla.redhat.com/show_bug.cgi?id=
jira-ticket-url: https://issues.redhat.com/browse/RHEL-

# Pagure login details (use either Pagure or Forgejo, not both)
pagure-repository: freeipa
# Create the token in https://pagure.io/freeipa/settings
# For token you need:
#   * Assign issue to someone
#   * Change the status of a ticket
#   * Comment on a ticket
#   * Create a new ticket
#   * Subscribe the user with this token to an issue
#   * Update an issue, status, comments, custom fields...
#   * Update the custom fields of an issue
#   * Update the milestone of an issue
pagure-token: "YOUR_PAGURE_TOKEN_HERE"
# workaround: pagure doesn't provide the tokens for users so we cannot
# dynamically detect username
username: username

# Forgejo login details (alternative to Pagure for issue tracking)
# forgejo-url: https://forgejo.example.com
# forgejo-repo: owner/repository
# forgejo-token: "YOUR_FORGEJO_TOKEN_HERE"

# Issue tracker operations (apply to both Pagure and Forgejo)
# update-issue options: yes/no/ask
update-issue: ask
# close-issue options: no/ask
close-issue: ask

# Jira configuration (secondary tracker; ticket keys come from the 'rhbz'
# custom field of Pagure/Forgejo issues, matched against jira-ticket-url)
# The Jira server URL is derived automatically from jira-ticket-url.
# jira-token: "your-personal-access-token"
# update-jira options: yes/no/ask
# update-jira: ask
# close-jira options: yes/no/ask (transitions the issue; default: no)
# close-jira: no
# Transition name to use when closing a Jira issue (default: Fixed)
# jira-close-transition: Fixed

# Mapping of logins to Git-style author lines
trac-username-map:
    abbra: Alexander Bokovoy <abokovoy@redhat.com>

# Command to run "git am" on the development tree (as argv list)
am-command: ["ssh", "ipa-devel-vm.local", "cd ~/freeipa/ ; git am -3"]

# Currently unused :(
browser: firefox

# GitHub configuration
# for the token, we require at least the 'repo', 'admin:org' group permissions
# and the 'user:email' and 'read:user' permissions
gh-token: "YOUR_GITHUB_TOKEN_HERE"
gh-repo: "freeipa/freeipa"
gh-fork-remote: "mygh"
"#;

#[derive(Parser)]
#[command(
    name = "ipatool",
    about = "Tool helping the IPA project processes",
    version
)]
struct Cli {
    /// Increase verbosity (repeat for more: -v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Configuration file
    #[arg(long, global = true, default_value = "~/.ipa/toolconf.yaml")]
    config: String,

    /// Do not add a Reviewed-By: line
    #[arg(long, global = true)]
    no_reviewer: bool,

    /// Do not push (dry run)
    #[arg(short = 'n', long, global = true)]
    dry_run: bool,

    /// Do not contact Pagure.io
    #[arg(long, global = true)]
    no_pagure: bool,

    /// Do not contact Forgejo
    #[arg(long, global = true)]
    no_forgejo: bool,

    /// Do not contact Jira
    #[arg(long, global = true)]
    no_jira: bool,

    /// Do not synchronize before pushing
    #[arg(long, global = true)]
    no_fetch: bool,

    /// Colorize output: auto, always, never
    #[arg(long, global = true, default_value = "auto")]
    color: String,

    /// Use cached data; queue changes for later sync
    #[arg(long, global = true)]
    offline: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print sample configuration file
    SampleConfig,

    /// Apply patches and push upstream
    Push {
        /// Branch to push to (can be repeated; detected from ticket if omitted)
        #[arg(short = 'b', long = "branch")]
        branches: Vec<String>,

        /// Reviewer name (can be repeated)
        #[arg(short = 'r', long = "reviewer")]
        reviewers: Vec<String>,

        /// Patch files or directories
        patches: Vec<String>,
    },

    /// Set yourself as reviewer for tickets
    StartReview {
        /// Force setting reviewer even if already set
        #[arg(short, long)]
        force: bool,

        /// Also apply the patches (like `ipatool am`)
        #[arg(long)]
        am: bool,

        /// Ticket number to update (can be repeated)
        #[arg(short = 't', long = "ticket")]
        tickets: Vec<u64>,

        /// Patch files or directories
        patches: Vec<String>,
    },

    /// Apply patches to the development tree via am-command
    Am {
        /// Patch files or directories
        patches: Vec<String>,
    },

    /// List GitHub pull requests
    PrList {
        /// Filter by state: open, closed, all (prefix with - to exclude)
        #[arg(short = 's', long = "state")]
        states: Vec<String>,

        /// Filter by label (prefix with - to exclude)
        #[arg(short = 'l', long = "label")]
        labels: Vec<String>,
    },

    /// ACK a pull request (add 'ack' label and comment)
    PrAck {
        /// Pull request number
        pr_id: u64,

        /// Comment text
        #[arg(short = 'c', long = "comment")]
        comment: Option<String>,
    },

    /// Reject a pull request (add 'rejected' label, comment, close)
    PrReject {
        /// Pull request number
        pr_id: u64,

        /// Reason for rejection
        #[arg(short = 'c', long = "comment", required = true)]
        comment: String,
    },

    /// Fetch PR patches and push them upstream
    PrPush {
        /// Pull request number
        pr_id: u64,

        /// Reviewer name (can be repeated)
        #[arg(short = 'r', long = "reviewer")]
        reviewers: Vec<String>,

        /// Backport to branch (can be repeated)
        #[arg(short = 'B', long = "backport")]
        backport: Vec<String>,

        /// Auto-backport based on PR labels
        #[arg(long)]
        autobackport: bool,
    },

    /// Backport a PR to specific branches
    Backport {
        /// Pull request number
        pr_id: u64,

        /// Target branch(es) for backport
        #[arg(short = 'b', long = "branch", required = true)]
        branches: Vec<String>,
    },

    /// Interactive TUI: browse PRs, inspect details, ACK/reject
    Tui {
        /// Which PRs to load: open, closed, or all
        #[arg(long, default_value = "open")]
        state: String,
    },

    /// Fetch PRs from GitHub and store them in the local cache for offline use
    CacheUpdate {
        /// Which PRs to cache: open, closed, or all
        #[arg(long, default_value = "open")]
        state: String,
    },

    /// List pending offline review actions without submitting them
    QueueList,

    /// Submit all pending offline review actions to GitHub
    QueueSubmit,
}

fn build_ctx(cli: &Cli) -> Result<Ctx> {
    let color = ColorMode::from_str(&cli.color);
    let out = Output::new(color);

    let config = match Config::load(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            // File-not-found is fine (user may not have created a config yet).
            // Any other error (parse failure, permission denied) is fatal.
            let is_not_found = e.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .map(|io| io.kind() == std::io::ErrorKind::NotFound)
                    .unwrap_or(false)
            });
            if is_not_found {
                Config::default()
            } else {
                return Err(e);
            }
        }
    };

    if cli.verbose > 0 {
        println!("Config:");
        println!("{}", config.sanitized_display());
    }

    // Build API clients
    let pagure = if !cli.no_pagure && config.has_pagure() {
        Some(Arc::new(api::pagure::PagureClient::new(
            &config.pagure_token,
            &config.pagure_repository,
        )?))
    } else {
        None
    };

    let forgejo = if !cli.no_forgejo && config.has_forgejo() {
        let (owner, repo) = config.forgejo_owner_repo()?;
        Some(Arc::new(api::forgejo::ForgejoClient::new(
            &config.forgejo_url,
            &config.forgejo_token,
            &owner,
            &repo,
        )?))
    } else {
        None
    };

    let jira = if !cli.no_jira && config.has_jira() {
        if let Some(server) = config.jira_server() {
            Some(Arc::new(api::jira::JiraClient::new(
                &server,
                &config.jira_token,
            )?))
        } else {
            None
        }
    } else {
        None
    };

    let github = if config.has_github() {
        let (owner, repo) = config.gh_owner_repo()?;
        Some(Arc::new(api::github::GitHubClient::new(
            &config.gh_token,
            &owner,
            &repo,
        )?))
    } else {
        None
    };

    let db = {
        let path = crate::config::expand_path(&config.db_path);
        match db::Database::open(&path.to_string_lossy()) {
            Ok(d) => Some(std::sync::Arc::new(d)),
            Err(e) => {
                eprintln!("Warning: could not open cache DB: {}", e);
                None
            }
        }
    };

    // Set GIT_COMMITTER_DATE so backported commits carry the current date.
    let mut git_env: HashMap<String, String> = std::env::vars().collect();
    let isodate_now = git::get_iso_date();
    if !isodate_now.is_empty() {
        git_env
            .entry("GIT_COMMITTER_DATE".to_string())
            .or_insert_with(|| isodate_now);
    }

    let style_path = tui_style::style_config_path(&cli.config);
    let tui_style = tui_style::TuiStyle::load_or_save_default(&style_path);

    let keys_path = tui_keys::keys_config_path(&cli.config);
    let tui_keys = tui_keys::TuiKeys::load_or_save_default(&keys_path);

    Ok(Ctx {
        config,
        pagure,
        forgejo,
        jira,
        github,
        verbosity: cli.verbose,
        dry_run: cli.dry_run,
        no_reviewer: cli.no_reviewer,
        no_fetch: cli.no_fetch,
        color: cli.color.clone(),
        out,
        push_info: None,
        git_env,
        offline: cli.offline,
        db,
        tui_style,
        tui_keys,
    })
}

fn main() {
    let cli = Cli::parse();

    // Handle sample-config without loading config
    if matches!(cli.command, Command::SampleConfig) {
        eprintln!(
            "\x1b[36mCopy the following to {}, and modify to taste:\x1b[0m",
            cli.config
        );
        eprintln!("\x1b[36m{:-<70}\x1b[0m", "---8<---");
        print!("{}", SAMPLE_CONFIG.trim());
        println!();
        eprintln!("\x1b[36m{:->70}\x1b[0m", "--->8---");
        return;
    }

    let mut ctx = match build_ctx(&cli) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("\x1b[31mError: {}\x1b[0m", e);
            std::process::exit(1);
        }
    };

    let result = run_command(&mut ctx, &cli.command);

    if let Err(e) = result {
        eprintln!("\x1b[31mError: {:#}\x1b[0m", e);
        std::process::exit(1);
    }
}

fn run_command(ctx: &mut Ctx, command: &Command) -> Result<()> {
    match command {
        Command::SampleConfig => unreachable!(),

        Command::Push {
            branches,
            reviewers,
            patches,
        } => commands::push::run(ctx, patches, branches, reviewers, false, &[]),

        Command::StartReview {
            force,
            am,
            tickets,
            patches,
        } => commands::start_review::run(ctx, *force, *am, tickets, patches),

        Command::Am { patches } => commands::am::run(ctx, patches),

        Command::PrList { states, labels } => commands::pr_list::run(ctx, states, labels),

        Command::PrAck { pr_id, comment } => commands::pr_ack::run(ctx, *pr_id, comment.as_deref()),

        Command::PrReject { pr_id, comment } => commands::pr_reject::run(ctx, *pr_id, comment),

        Command::PrPush {
            pr_id,
            reviewers,
            backport,
            autobackport,
        } => commands::pr_push::run(ctx, *pr_id, reviewers, backport, *autobackport),

        Command::Backport { pr_id, branches } => {
            commands::backport::run_backport_cmd(ctx, *pr_id, branches)
        }

        Command::Tui { state } => commands::interactive::run(ctx, state),

        Command::CacheUpdate { state } => commands::cache_update::run(ctx, state),

        Command::QueueList => commands::queue_submit::run_list(ctx),

        Command::QueueSubmit => commands::queue_submit::run_submit(ctx),
    }
}
