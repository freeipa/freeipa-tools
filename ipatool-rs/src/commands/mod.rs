use anyhow::{bail, Result};
use regex::Regex;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::api::{
    forgejo::{ForgejoClient, ForgejoTicket},
    github::{GitHubClient, GitHubTicket},
    jira::JiraClient,
    pagure::{PagureClient, PagureTicket},
};
use crate::config::{Config, IssueTracker};
use crate::output::{ask_yn, prompt, Output};

pub(crate) use crate::api::types::TicketOps;

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

/// Fallback expected remote server when no forge-specific URL is configured.
pub const GIT_REMOTE_SERVER: &str = "codeberg.org";

static REVIEWER_RE: OnceLock<Regex> = OnceLock::new();

fn get_reviewer_re() -> &'static Regex {
    REVIEWER_RE.get_or_init(|| {
        Regex::new(r"^\w+ [^<]+ <.*@.*\..*>$")
            .expect("REVIEWER_RE pattern is valid; this is a bug if it fails")
    })
}

/// Milestone to branches mapping (prefix → list of branches)
pub fn milestone_branches(milestone: &str) -> Option<Vec<String>> {
    // Prefixes are chosen so that starts_with replicates the original regex semantics:
    //   "FreeIPA 3.3." requires at least one character after the second dot (like ^FreeIPA 3\.3\..*)
    //   "FreeIPA 4.4"  matches "FreeIPA 4.4" and "FreeIPA 4.4.x" (like ^FreeIPA 4\.4.*)
    let mappings: &[(&str, &[&str])] = &[
        ("FreeIPA 3.3.", &["master", "ipa-4-1", "ipa-4-0", "ipa-3-3"]),
        ("FreeIPA 4.4", &["master", "ipa-4-5", "ipa-4-4"]),
        ("FreeIPA 4.5", &["master", "ipa-4-5"]),
        ("FreeIPA 4.6", &["master"]),
        ("FreeIPA 4.7", &["master"]),
    ];
    for (prefix, branches) in mappings {
        if milestone.starts_with(prefix) {
            return Some(branches.iter().map(|s| s.to_string()).collect());
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

/// Ticket abstraction that works with Pagure, Forgejo, or GitHub Issues
pub(crate) enum Ticket {
    Pagure(PagureTicket),
    Forgejo(ForgejoTicket),
    GitHub(GitHubTicket),
}

impl TicketOps for Ticket {
    fn number(&self) -> u64 {
        match self {
            Ticket::Pagure(t) => t.number,
            Ticket::Forgejo(t) => t.number,
            Ticket::GitHub(t) => t.number,
        }
    }
    fn reviewer(&self) -> Result<Option<String>> {
        match self { Ticket::Pagure(t) => t.reviewer(), Ticket::Forgejo(t) => t.reviewer(), Ticket::GitHub(t) => t.reviewer() }
    }
    fn rhbz(&self) -> Result<Option<String>> {
        match self { Ticket::Pagure(t) => t.rhbz(), Ticket::Forgejo(t) => t.rhbz(), Ticket::GitHub(t) => t.rhbz() }
    }
    fn milestone(&self) -> Result<Option<String>> {
        match self { Ticket::Pagure(t) => t.milestone(), Ticket::Forgejo(t) => t.milestone(), Ticket::GitHub(t) => t.milestone() }
    }
    fn title(&self) -> Result<String> {
        match self { Ticket::Pagure(t) => t.title(), Ticket::Forgejo(t) => t.title(), Ticket::GitHub(t) => t.title() }
    }
    fn is_closed(&self) -> Result<bool> {
        match self { Ticket::Pagure(t) => t.is_closed(), Ticket::Forgejo(t) => t.is_closed(), Ticket::GitHub(t) => t.is_closed() }
    }
    fn comment(&self, text: &str) -> Result<()> {
        match self { Ticket::Pagure(t) => t.comment(text), Ticket::Forgejo(t) => t.comment(text), Ticket::GitHub(t) => t.comment(text) }
    }
    fn close(&self) -> Result<()> {
        match self { Ticket::Pagure(t) => t.close(), Ticket::Forgejo(t) => t.close(), Ticket::GitHub(t) => t.close() }
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
    /// Active profile name from --profile (empty string for the default profile).
    /// Used as the cache namespace so different profiles don't share cached PR data.
    pub profile: String,
    /// Which issue tracker holds the linked tickets (set by --profile or default)
    pub issue_tracker: IssueTracker,
    /// Unified PR client for the active pr_source (None if no PR client is configured)
    pub pr_client: Option<Arc<pr_client::PrClient>>,
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
            IssueTracker::Forgejo => self.forgejo.as_ref().map(|f| {
                Ticket::Forgejo(ForgejoTicket::new(
                    Arc::clone(f),
                    number,
                    self.config.forgejo_comment_field_prefix.clone(),
                ))
            }),
            IssueTracker::GitHub => self.github.as_ref().map(|gh| {
                Ticket::GitHub(GitHubTicket::new(
                    Arc::clone(gh),
                    number,
                    self.config.github_comment_field_prefix.clone(),
                ))
            }),
        }
    }

    pub fn has_tracker(&self) -> bool {
        match self.issue_tracker {
            IssueTracker::Pagure => self.pagure.is_some(),
            IssueTracker::Forgejo => self.forgejo.is_some(),
            IssueTracker::GitHub => self.github.is_some(),
        }
    }

    /// Return the active PR client or a helpful error.
    pub fn pr_client_or_err(&self) -> Result<&Arc<pr_client::PrClient>> {
        self.pr_client.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "No PR client configured — check gh-token/gh-repo (GitHub) \
                 or forgejo-url/forgejo-repo/forgejo-token (Forgejo) in the config, \
                 and ensure the selected --profile has a matching pr-source"
            )
        })
    }

    /// Return the expected git remote server hostname for the active forge.
    /// Used by `verify_remote_url` to warn when pushing to an unexpected host.
    fn expected_remote_server(&self) -> &str {
        // Check the active forge via pr_client first.
        if let Some(client) = &self.pr_client {
            match client.provider() {
                crate::db::Provider::GitHub => return "github.com",
                crate::db::Provider::Forgejo => {
                    if !self.config.forgejo_url.is_empty() {
                        return self
                            .config
                            .forgejo_url
                            .trim_start_matches("https://")
                            .trim_start_matches("http://")
                            .trim_end_matches('/');
                    }
                }
                crate::db::Provider::Pagure => return GIT_REMOTE_SERVER,
            }
        }
        // Fallback: derive from configured URLs.
        if !self.config.forgejo_url.is_empty() {
            return self
                .config
                .forgejo_url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches('/');
        }
        GIT_REMOTE_SERVER
    }

    pub fn verify_remote_url(&self) -> Result<()> {
        let remote = &self.config.remote;
        let url = crate::git::remote_get_url(remote, &self.git_env, self.verbosity)?;
        let expected = self.expected_remote_server();
        if !url.contains(expected) {
            self.out.print_red(&format!(
                "!!! WARNING !!! not pushing to {} git repo",
                expected
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
    if get_reviewer_re().is_match(reviewer) {
        return Ok(reviewer.to_string());
    }
    let remote_master = format!("{}/master", ctx.config.remote);
    let names = crate::git::shortlog_sen(&remote_master, &ctx.git_env, ctx.verbosity)?;
    let re = get_reviewer_re();
    let reviewer_lower = reviewer.to_lowercase();
    let matches: Vec<String> = names
        .into_iter()
        .filter(|n| re.is_match(n) && n.to_lowercase().contains(&reviewer_lower))
        .collect();

    match matches.len() {
        0 => bail!("Reviewer '{}' not found in git shortlog", reviewer),
        1 => Ok(matches
            .into_iter()
            .next()
            .expect("exactly one match confirmed above; None here is a logic error")),
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
            match ticket.reviewer() {
                Ok(Some(r)) => {
                    found.insert(r);
                }
                Ok(None) => {
                    // No reviewer set in this ticket; continue checking others.
                }
                Err(e) => {
                    // Surface network/parse errors as a warning rather than
                    // silently treating them as "no reviewer set".
                    ctx.out.print_yellow(&format!(
                        "Warning: could not retrieve reviewer from ticket #{}: {}",
                        ticket.number(),
                        e
                    ));
                }
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

/// Cleanup git state after push attempt.
///
/// Each git step is attempted independently; failures are logged as warnings
/// so that subsequent cleanup steps still run.
pub fn git_cleanup(ctx: &Ctx, old_branch: &str) {
    println!("Cleaning up");
    if let Err(e) = crate::git::am_abort(&ctx.git_env) {
        eprintln!("Warning: git am --abort failed during cleanup: {}", e);
    }
    if let Err(e) = crate::git::reset_hard(&ctx.git_env) {
        eprintln!("Warning: git reset --hard failed during cleanup: {}", e);
    }
    if let Err(e) = crate::git::checkout_branch(old_branch, &ctx.git_env) {
        eprintln!(
            "Warning: git checkout '{}' failed during cleanup (repository may be on wrong branch): {}",
            old_branch, e
        );
    }
    if let Err(e) = crate::git::clean_fxd(&ctx.git_env) {
        eprintln!("Warning: git clean -fxd failed during cleanup: {}", e);
    }
}

/// Update a ticket with push info
pub fn update_issue(ctx: &Ctx, ticket: &impl TicketOps) {
    let Some(push_info) = &ctx.push_info else {
        return;
    };
    let config_val = ctx.config.update_issue.as_str();
    let title = match ticket.title() {
        Ok(t) => t,
        Err(e) => {
            ctx.out.print_red(&format!(
                "Cannot retrieve ticket #{} from issue tracker: {}",
                ticket.number(),
                e
            ));
            ctx.out
                .print_yellow("Please update the issue manually with the commit info above");
            return;
        }
    };
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
pub fn close_issue(ctx: &Ctx, ticket: &impl TicketOps, has_backport: bool) {
    if has_backport {
        return;
    }
    let is_closed = match ticket.is_closed() {
        Ok(v) => v,
        Err(e) => {
            ctx.out.print_red(&format!(
                "Cannot retrieve ticket #{} from issue tracker: {}",
                ticket.number(),
                e
            ));
            ctx.out
                .print_yellow("Please check and close the issue manually if appropriate");
            return;
        }
    };
    if is_closed {
        println!("Issue already closed.");
        return;
    }
    let config_val = ctx.config.close_issue.as_str();
    let title = match ticket.title() {
        Ok(t) => t,
        Err(e) => {
            ctx.out.print_red(&format!(
                "Cannot retrieve ticket #{} from issue tracker: {}",
                ticket.number(),
                e
            ));
            ctx.out
                .print_yellow("Please close the issue manually if appropriate");
            return;
        }
    };
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
