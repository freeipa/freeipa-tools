use anyhow::{bail, Result};
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;

use crate::api::{
    forgejo::{ForgejoClient, ForgejoTicket},
    github::GitHubClient,
    jira::JiraClient,
    pagure::{PagureClient, PagureTicket},
};
use crate::config::{Config, IssueTracker};
use crate::output::{ask_yn, prompt, Output};

pub mod am;
pub mod backport;
pub mod cache_update;
pub mod interactive;
pub mod pr_ack;
pub mod pr_client;
pub mod pr_list;
pub mod pr_push;
pub mod pr_reject;
pub mod push;
pub mod queue_submit;
pub mod start_review;

pub const GIT_REMOTE_SERVER: &str = "pagure.io";

/// Milestone to branches mapping (regex → list of branches)
pub fn milestone_branches(milestone: &str) -> Option<Vec<String>> {
    let mappings: &[(&str, &[&str])] = &[
        (
            r"^FreeIPA 3\.3\..*",
            &["master", "ipa-4-1", "ipa-4-0", "ipa-3-3"],
        ),
        (r"^FreeIPA 4\.4.*", &["master", "ipa-4-5", "ipa-4-4"]),
        (r"^FreeIPA 4\.5.*", &["master", "ipa-4-5"]),
        (r"^FreeIPA 4\.6.*", &["master"]),
        (r"^FreeIPA 4\.7.*", &["master"]),
    ];
    for (pattern, branches) in mappings {
        if let Ok(re) = Regex::new(pattern) {
            if re.is_match(milestone) {
                return Some(branches.iter().map(|s| s.to_string()).collect());
            }
        }
    }
    None
}

#[cfg(test)]
mod milestone_tests {
    use super::milestone_branches;

    #[test]
    fn test_milestone_33_with_patch() {
        let branches = milestone_branches("FreeIPA 3.3.0").unwrap();
        assert_eq!(branches.len(), 4);
        assert!(branches.contains(&"master".to_string()));
        assert!(branches.contains(&"ipa-4-1".to_string()));
        assert!(branches.contains(&"ipa-4-0".to_string()));
        assert!(branches.contains(&"ipa-3-3".to_string()));
    }

    #[test]
    fn test_milestone_33_subversion() {
        let branches = milestone_branches("FreeIPA 3.3.5").unwrap();
        assert_eq!(branches.len(), 4);
    }

    #[test]
    fn test_milestone_33_requires_patch_component() {
        // Pattern is "^FreeIPA 3\.3\..*" — needs at least one char after the second dot
        assert!(milestone_branches("FreeIPA 3.3").is_none());
    }

    #[test]
    fn test_milestone_44() {
        let branches = milestone_branches("FreeIPA 4.4").unwrap();
        assert_eq!(branches.len(), 3);
        assert!(branches.contains(&"master".to_string()));
        assert!(branches.contains(&"ipa-4-5".to_string()));
        assert!(branches.contains(&"ipa-4-4".to_string()));
    }

    #[test]
    fn test_milestone_44_with_patch() {
        let branches = milestone_branches("FreeIPA 4.4.3").unwrap();
        assert_eq!(branches.len(), 3);
    }

    #[test]
    fn test_milestone_45() {
        let branches = milestone_branches("FreeIPA 4.5").unwrap();
        assert_eq!(branches, vec!["master".to_string(), "ipa-4-5".to_string()]);
    }

    #[test]
    fn test_milestone_46() {
        let branches = milestone_branches("FreeIPA 4.6").unwrap();
        assert_eq!(branches, vec!["master".to_string()]);
    }

    #[test]
    fn test_milestone_47() {
        let branches = milestone_branches("FreeIPA 4.7").unwrap();
        assert_eq!(branches, vec!["master".to_string()]);
    }

    #[test]
    fn test_milestone_no_match_unknown() {
        assert!(milestone_branches("Unknown 1.0").is_none());
    }

    #[test]
    fn test_milestone_no_match_empty() {
        assert!(milestone_branches("").is_none());
    }

    #[test]
    fn test_milestone_no_match_future_version() {
        // 5.x is not mapped
        assert!(milestone_branches("FreeIPA 5.0").is_none());
    }

    #[test]
    fn test_milestone_no_match_partial_prefix() {
        assert!(milestone_branches("FreeIPA 4").is_none());
        assert!(milestone_branches("FreeIPA").is_none());
    }
}

// ── GitHub Issues as a ticket backend ────────────────────────────────────────

pub struct GitHubTicket {
    pub client: Arc<GitHubClient>,
    pub number: u64,
    data: std::sync::OnceLock<crate::api::github::GitHubIssue>,
}

impl GitHubTicket {
    pub fn new(client: Arc<GitHubClient>, number: u64) -> Self {
        GitHubTicket {
            client,
            number,
            data: std::sync::OnceLock::new(),
        }
    }

    pub fn data(&self) -> Result<&crate::api::github::GitHubIssue> {
        if let Some(d) = self.data.get() {
            return Ok(d);
        }
        println!("Retrieving GitHub issue #{}", self.number);
        let issue = self.client.get_issue(self.number)?;
        let _ = self.data.set(issue);
        Ok(self.data.get().unwrap())
    }

    pub fn title(&self) -> Result<String> {
        Ok(self.data()?.title.clone())
    }

    pub fn is_closed(&self) -> Result<bool> {
        Ok(self.data()?.is_closed())
    }

    pub fn comment(&self, text: &str) -> Result<()> {
        self.client.create_comment(self.number, text)
    }

    pub fn close(&self) -> Result<()> {
        self.client.close_issue(self.number)
    }

    pub fn milestone(&self) -> Result<Option<String>> {
        Ok(self.data()?.milestone.as_ref().map(|m| m.title.clone()))
    }
}

/// Ticket abstraction that works with Pagure, Forgejo, or GitHub Issues
pub enum Ticket {
    Pagure(PagureTicket),
    Forgejo(ForgejoTicket),
    GitHub(GitHubTicket),
}

impl Ticket {
    pub fn number(&self) -> u64 {
        match self {
            Ticket::Pagure(t) => t.number,
            Ticket::Forgejo(t) => t.number,
            Ticket::GitHub(t) => t.number,
        }
    }

    pub fn reviewer(&self) -> Result<Option<String>> {
        match self {
            Ticket::Pagure(t) => t.reviewer(),
            Ticket::Forgejo(t) => Ok(t.reviewer()),
            Ticket::GitHub(_) => Ok(None), // GitHub Issues have no reviewer custom field
        }
    }

    pub fn rhbz(&self) -> Result<Option<String>> {
        match self {
            Ticket::Pagure(t) => t.rhbz(),
            Ticket::Forgejo(t) => Ok(t.rhbz()),
            Ticket::GitHub(_) => Ok(None), // GitHub Issues have no rhbz custom field
        }
    }

    pub fn milestone(&self) -> Result<Option<String>> {
        match self {
            Ticket::Pagure(t) => t.milestone(),
            Ticket::Forgejo(t) => t.milestone(),
            Ticket::GitHub(t) => t.milestone(),
        }
    }

    pub fn title(&self) -> Result<String> {
        match self {
            Ticket::Pagure(t) => t.title(),
            Ticket::Forgejo(t) => t.title(),
            Ticket::GitHub(t) => t.title(),
        }
    }

    pub fn is_closed(&self) -> Result<bool> {
        match self {
            Ticket::Pagure(t) => t.is_closed(),
            Ticket::Forgejo(t) => t.is_closed(),
            Ticket::GitHub(t) => t.is_closed(),
        }
    }

    pub fn comment(&self, text: &str) -> Result<()> {
        match self {
            Ticket::Pagure(t) => t.comment(text),
            Ticket::Forgejo(t) => t.comment(text),
            Ticket::GitHub(t) => t.comment(text),
        }
    }

    pub fn close(&self) -> Result<()> {
        match self {
            Ticket::Pagure(t) => t.close(),
            Ticket::Forgejo(t) => t.close(),
            Ticket::GitHub(t) => t.close(),
        }
    }
}

/// Central context holding all state needed by commands
pub struct Ctx {
    pub config: Config,
    pub pagure: Option<Arc<PagureClient>>,
    pub forgejo: Option<Arc<ForgejoClient>>,
    pub jira: Option<Arc<JiraClient>>,
    pub github: Option<Arc<GitHubClient>>,
    pub verbosity: u8,
    pub dry_run: bool,
    pub no_reviewer: bool,
    pub no_fetch: bool,
    pub color: String,
    pub out: Output,
    // Populated during push for post-push operations
    pub push_info: Option<PushInfo>,
    // git env (GIT_COMMITTER_DATE)
    pub git_env: HashMap<String, String>,
    pub offline: bool,
    pub db: Option<Arc<crate::db::Database>>,
    pub tui_style: crate::tui_style::TuiStyle,
    pub tui_keys: crate::tui_keys::TuiKeys,
    /// Which issue tracker holds the linked tickets (set by --profile or default)
    pub issue_tracker: IssueTracker,
}

#[derive(Debug, Default, Clone)]
pub struct PushInfo {
    pub pushed: bool,
    pub pagure_comment: String,
    pub bugzilla_comment: String,
    pub jira_urls: Vec<String>,
}

impl Ctx {
    pub fn make_ticket(&self, number: u64) -> Option<Ticket> {
        match self.issue_tracker {
            IssueTracker::Pagure => self
                .pagure
                .as_ref()
                .map(|p| Ticket::Pagure(PagureTicket::new(Arc::clone(p), number))),
            IssueTracker::Forgejo => self
                .forgejo
                .as_ref()
                .map(|f| Ticket::Forgejo(ForgejoTicket::new(Arc::clone(f), number))),
            IssueTracker::GitHub => self
                .github
                .as_ref()
                .map(|gh| Ticket::GitHub(GitHubTicket::new(Arc::clone(gh), number))),
        }
    }

    pub fn has_tracker(&self) -> bool {
        match self.issue_tracker {
            IssueTracker::Pagure => self.pagure.is_some(),
            IssueTracker::Forgejo => self.forgejo.is_some(),
            IssueTracker::GitHub => self.github.is_some(),
        }
    }

    pub fn verify_remote_url(&self) -> Result<()> {
        let remote = &self.config.remote;
        let url = crate::git::remote_get_url(remote, &self.git_env, self.verbosity)?;
        if !url.contains(GIT_REMOTE_SERVER) {
            self.out.print_red(&format!(
                "!!! WARNING !!! not pushing to {} git repo",
                GIT_REMOTE_SERVER
            ));
            let response = prompt(&format!("Push to \"{}\"? [y/n] ", url));
            if response.to_lowercase() != "y" {
                bail!("Aborting push");
            }
        }
        Ok(())
    }
}

/// Resolve reviewer name to full "Name <email>" format
pub fn normalize_reviewer(ctx: &Ctx, reviewer: &str) -> Result<String> {
    // If already full format, use as-is
    if let Ok(re) = Regex::new(r"^\w+ [^<]+ <.*@.*\..*>$") {
        if re.is_match(reviewer) {
            return Ok(reviewer.to_string());
        }
    }
    let remote_master = format!("{}/master", ctx.config.remote);
    let names = crate::git::shortlog_sen(&remote_master, &ctx.git_env, ctx.verbosity)?;
    let name_re = Regex::new(r"^\w+ [^<]+ <.*@.*\..*>$").unwrap();
    let reviewer_lower = reviewer.to_lowercase();
    let matches: Vec<String> = names
        .into_iter()
        .filter(|n| name_re.is_match(n) && n.to_lowercase().contains(&reviewer_lower))
        .collect();

    match matches.len() {
        0 => bail!("Reviewer '{}' not found in git shortlog", reviewer),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => {
            ctx.out
                .print_red(&format!("Reviewer '{}' could be:", reviewer));
            for name in &matches {
                println!("- {}", name);
            }
            bail!("Multiple matches found for reviewer '{}'", reviewer)
        }
    }
}

/// Get reviewers from options or tickets
pub fn get_reviewers(
    ctx: &Ctx,
    reviewer_args: &[String],
    tickets: &[Ticket],
) -> Result<Vec<String>> {
    if ctx.no_reviewer {
        return Ok(vec![]);
    }

    let mut reviewers: Vec<String> = reviewer_args.to_vec();

    if ctx.has_tracker() && reviewers.is_empty() {
        let mut found = std::collections::HashSet::new();
        for ticket in tickets {
            if let Ok(Some(r)) = ticket.reviewer() {
                found.insert(r);
            }
        }
        if found.len() > 1 {
            println!(
                "Reviewers found: {}",
                found.iter().cloned().collect::<Vec<_>>().join(", ")
            );
            bail!("Too many reviewers found in ticket(s), specify --reviewer explicitly");
        }
        if found.is_empty() {
            bail!("No reviewer found in ticket(s), specify --reviewer explicitly");
        }
        let username_map = &ctx.config.trac_username_map;
        reviewers = found
            .into_iter()
            .map(|r| username_map.get(&r).cloned().unwrap_or(r))
            .collect();
    }

    if reviewers.is_empty() {
        bail!("No reviewer found, please specify --reviewer");
    }

    reviewers
        .iter()
        .map(|r| normalize_reviewer(ctx, r))
        .collect()
}

/// Apply patches to a branch, return resulting SHA
pub fn apply_patches_to_branch(
    ctx: &Ctx,
    patches: &[crate::patch::Patch],
    branch: &str,
    die_on_fail: bool,
) -> Result<String> {
    crate::git::checkout_remote_branch(&ctx.config.remote, branch, &ctx.git_env, ctx.verbosity)?;

    for patch in patches {
        println!("Applying to {}: {}", branch, patch.subject);
        let content = patch.content();
        let result = crate::git::git_am(&content, &ctx.git_env, ctx.verbosity)?;
        if result.returncode != 0 {
            if die_on_fail {
                bail!("git am failed for patch: {}", patch.subject);
            } else {
                return Err(anyhow::anyhow!("git am failed: {}", result.stderr));
            }
        }
    }

    let sha = crate::git::rev_parse_head(&ctx.git_env, ctx.verbosity)?;
    if ctx.verbosity > 0 {
        println!("Resulting hash: {}", sha);
    }
    Ok(sha)
}

/// Cleanup git state after push attempt
pub fn git_cleanup(ctx: &Ctx, old_branch: &str) {
    println!("Cleaning up");
    crate::git::am_abort(&ctx.git_env);
    crate::git::reset_hard(&ctx.git_env);
    crate::git::checkout_branch(old_branch, &ctx.git_env);
    crate::git::clean_fxd(&ctx.git_env);
}

/// Update a ticket with push info
pub fn update_issue(ctx: &Ctx, ticket: &Ticket) {
    let Some(push_info) = &ctx.push_info else {
        return;
    };
    let config_val = ctx.config.update_issue.as_str();
    let Ok(title) = ticket.title() else { return };
    let do_comment = ask_yn(
        config_val,
        "update-issue",
        &format!(
            "Update issue \"#{}: {}\" with commit info?",
            ticket.number(),
            title
        ),
    );
    if do_comment {
        match ticket.comment(&push_info.pagure_comment) {
            Ok(()) => ctx.out.print_green("Comment added"),
            Err(e) => {
                ctx.out.print_red(&format!("Comment failed: {}", e));
                ctx.out.print_yellow("Please update issue manually");
            }
        }
    }
}

/// Close a ticket if configured and not already closed
pub fn close_issue(ctx: &Ctx, ticket: &Ticket, has_backport: bool) {
    if has_backport {
        return;
    }
    let Ok(is_closed) = ticket.is_closed() else {
        return;
    };
    if is_closed {
        println!("Issue already closed.");
        return;
    }
    let config_val = ctx.config.close_issue.as_str();
    let Ok(title) = ticket.title() else { return };
    let do_close = ask_yn(
        config_val,
        "close-issue",
        &format!("Close issue \"#{}: {}\"?", ticket.number(), title),
    );
    if do_close {
        match ticket.close() {
            Ok(()) => ctx.out.print_green("Issue closed"),
            Err(e) => {
                ctx.out
                    .print_red(&format!("Failed to close the issue: {}", e));
                ctx.out.print_yellow("Please close the issue manually");
            }
        }
    }
}

/// Update Jira tickets with commit info
pub fn update_jira_issues(ctx: &Ctx) {
    let Some(jira) = &ctx.jira else { return };
    let Some(push_info) = &ctx.push_info else {
        return;
    };
    if push_info.jira_urls.is_empty() {
        return;
    }

    let jira_ticket_url = &ctx.config.jira_ticket_url;
    let browse_prefix = if jira_ticket_url.contains("/browse/") {
        format!(
            "{}/browse/",
            jira_ticket_url.split("/browse/").next().unwrap_or("")
        )
    } else {
        return;
    };

    let update_jira = ctx.config.update_jira.as_str();
    let close_jira = ctx.config.close_jira.as_str();
    let transition_name = ctx.config.jira_close_transition.as_str();

    for url in &push_info.jira_urls {
        let issue_key = url.trim_start_matches(&browse_prefix);

        let do_comment = ask_yn(
            update_jira,
            "update-jira",
            &format!("Add commit info comment to Jira {}?", issue_key),
        );
        if do_comment {
            match jira.add_comment(issue_key, &push_info.bugzilla_comment) {
                Ok(()) => ctx
                    .out
                    .print_green(&format!("Comment added to Jira {}", issue_key)),
                Err(e) => {
                    ctx.out
                        .print_red(&format!("Failed to comment on Jira {}: {}", issue_key, e));
                    ctx.out.print_yellow("Please update Jira manually");
                }
            }
        }

        let do_close = ask_yn(
            close_jira,
            "close-jira",
            &format!("Transition Jira {} to \"{}\"?", issue_key, transition_name),
        );
        if do_close {
            match jira.get_transitions(issue_key) {
                Err(e) => {
                    ctx.out.print_red(&format!(
                        "Failed to get transitions for Jira {}: {}",
                        issue_key, e
                    ));
                }
                Ok(transitions) => {
                    let transition_id = transitions
                        .iter()
                        .find(|t| t.name == transition_name)
                        .map(|t| t.id.clone());
                    match transition_id {
                        None => {
                            let available: Vec<&str> =
                                transitions.iter().map(|t| t.name.as_str()).collect();
                            ctx.out.print_red(&format!(
                                "Transition \"{}\" not found for {}. Available: {}",
                                transition_name,
                                issue_key,
                                available.join(", ")
                            ));
                        }
                        Some(tid) => match jira.transition_issue(issue_key, &tid) {
                            Ok(()) => ctx.out.print_green(&format!(
                                "Jira {} transitioned to \"{}\"",
                                issue_key, transition_name
                            )),
                            Err(e) => {
                                ctx.out.print_red(&format!(
                                    "Failed to transition Jira {}: {}",
                                    issue_key, e
                                ));
                                ctx.out.print_yellow("Please transition Jira manually");
                            }
                        },
                    }
                }
            }
        }
    }
}
