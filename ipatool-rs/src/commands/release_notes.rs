use anyhow::{bail, Result};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::commands::Ctx;
use crate::config::IssueTracker;
use crate::git_log::{self, GitLogResult};

static RHBZ_RE: OnceLock<Regex> = OnceLock::new();

fn rhbz_re() -> &'static Regex {
    RHBZ_RE.get_or_init(|| {
        Regex::new(r"https://bugzilla\.redhat\.com/show_bug\.cgi\?id=(\d+)").unwrap()
    })
}

#[derive(Debug, Clone, PartialEq)]
enum TicketCategory {
    Enhancement,
    KnownIssue,
    BugFix,
}

#[derive(Debug, Clone)]
struct ReleaseTicket {
    number: u64,
    title: String,
    category: TicketCategory,
    changelog: Vec<String>,
    rhbz: Option<String>,
}

pub struct ReleaseNotesParams<'a> {
    pub version: &'a str,
    pub release_date: &'a str,
    pub prev_version: &'a str,
    pub major_version: &'a str,
    pub revision_range: &'a str,
    pub milestone: &'a str,
    pub additional_milestones: &'a [String],
    pub links: bool,
    pub wiki: bool,
    pub rst: bool,
    pub no_milestones: bool,
    pub repo_path: Option<&'a str>,
}

struct FormatConfig<'a> {
    ticket_url: &'a str,
    commit_url: &'a str,
    bugzilla_bug_url: &'a str,
    links: bool,
}

pub fn run(ctx: &Ctx, params: &ReleaseNotesParams<'_>) -> Result<()> {
    let repo = params.repo_path.unwrap_or(&ctx.config.clean_repo_path);
    if repo.is_empty() {
        bail!("No git repository path specified (use --repo or set clean-repo-path in config)");
    }
    let repo = crate::config::expand_path(repo)
        .to_string_lossy()
        .to_string();

    let git_output =
        git_log::run_git_log(params.revision_range, &repo, &ctx.git_env, ctx.verbosity)?;
    let git = git_log::parse_git_log(
        &git_output,
        &ctx.config.ticket_url,
        &ctx.config.legacy_ticket_url,
    );

    let mut tickets: HashMap<u64, ReleaseTicket> = HashMap::new();

    if !params.no_milestones && ctx.has_tracker() {
        let primary = fetch_milestone_tickets(ctx, params.milestone)?;
        for t in primary {
            tickets.entry(t.number).or_insert(t);
        }

        let commit_ticket_ids = collect_commit_ticket_ids(&git);

        for ms in params.additional_milestones {
            let extra = fetch_milestone_tickets(ctx, ms)?;
            for t in extra {
                if commit_ticket_ids.contains(&t.number) {
                    tickets.entry(t.number).or_insert(t);
                }
            }
        }
    }

    if ctx.has_tracker() {
        let existing_ids: HashSet<u64> = tickets.keys().copied().collect();
        for commit in &git.commits {
            for &ticket_id in &commit.tickets {
                if !existing_ids.contains(&ticket_id) {
                    match fetch_single_ticket(ctx, ticket_id) {
                        Ok(t) => {
                            tickets.entry(t.number).or_insert(t);
                        }
                        Err(e) => {
                            eprintln!("Warning: could not fetch ticket #{}: {}", ticket_id, e);
                        }
                    }
                }
            }
        }
    }

    for commit in &git.commits {
        if commit.release_notes.is_empty() {
            continue;
        }
        for &ticket_id in &commit.tickets {
            if let Some(t) = tickets.get_mut(&ticket_id) {
                t.changelog.extend(commit.release_notes.clone());
            }
        }
    }

    let mut sorted_tickets: Vec<ReleaseTicket> = tickets.into_values().collect();
    sorted_tickets.sort_by_key(|t| t.number);

    let bugs: Vec<&ReleaseTicket> = sorted_tickets
        .iter()
        .filter(|t| t.category == TicketCategory::BugFix)
        .collect();

    let fmt = FormatConfig {
        ticket_url: &ctx.config.ticket_url,
        commit_url: &ctx.config.commit_url,
        bugzilla_bug_url: &ctx.config.bugzilla_bug_url,
        links: params.links,
    };

    if params.wiki {
        print_wiki(&sorted_tickets, &bugs, &git, params, &fmt);
    } else if params.rst {
        print_rst(&sorted_tickets, &bugs, &git, params, &fmt);
    } else {
        print_markdown(&sorted_tickets, &bugs, &git, params, &fmt);
    }

    Ok(())
}

fn collect_commit_ticket_ids(git: &GitLogResult) -> HashSet<u64> {
    let mut ids = HashSet::new();
    for commit in &git.commits {
        ids.extend(&commit.tickets);
    }
    ids
}

fn fetch_milestone_tickets(ctx: &Ctx, milestone: &str) -> Result<Vec<ReleaseTicket>> {
    match ctx.issue_tracker {
        IssueTracker::Pagure => fetch_pagure_milestone(ctx, milestone),
        IssueTracker::GitHub => fetch_github_milestone(ctx, milestone),
        IssueTracker::Forgejo => fetch_forgejo_milestone(ctx, milestone),
    }
}

fn fetch_pagure_milestone(ctx: &Ctx, milestone: &str) -> Result<Vec<ReleaseTicket>> {
    let pagure = ctx
        .pagure
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Pagure client not configured"))?;
    let issues = pagure.list_issues("Closed", &[milestone])?;
    let mut tickets = Vec::new();
    for issue in issues {
        let close_status = issue.close_status.as_deref().unwrap_or("").to_lowercase();
        if close_status != "fixed" {
            continue;
        }
        tickets.push(pagure_issue_to_release_ticket(&issue));
    }
    Ok(tickets)
}

fn fetch_github_milestone(ctx: &Ctx, milestone: &str) -> Result<Vec<ReleaseTicket>> {
    let github = ctx
        .github
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("GitHub client not configured"))?;
    let issues = github.list_issues_by_milestone("closed", milestone)?;
    let tickets = issues.iter().map(github_issue_to_release_ticket).collect();
    Ok(tickets)
}

fn fetch_forgejo_milestone(ctx: &Ctx, milestone: &str) -> Result<Vec<ReleaseTicket>> {
    let forgejo = ctx
        .forgejo
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Forgejo client not configured"))?;
    let issues = forgejo.list_issues_by_milestone("closed", milestone)?;
    let tickets = issues.iter().map(forgejo_issue_to_release_ticket).collect();
    Ok(tickets)
}

fn pagure_issue_to_release_ticket(issue: &crate::api::pagure::PagureIssue) -> ReleaseTicket {
    let has_rfe = issue.tags.iter().any(|t| t.eq_ignore_ascii_case("rfe"));
    let knownissue = issue.custom_field("knownissue").unwrap_or("false");
    let is_known_issue = knownissue != "false";

    let category = if is_known_issue {
        TicketCategory::KnownIssue
    } else if has_rfe {
        TicketCategory::Enhancement
    } else {
        TicketCategory::BugFix
    };

    let changelog = issue
        .custom_field("changelog")
        .map(|s| vec![s.to_string()])
        .unwrap_or_default();

    let rhbz = issue.custom_field("rhbz").map(|s| s.to_string());

    ReleaseTicket {
        number: issue.id,
        title: issue.title.clone(),
        category,
        changelog,
        rhbz,
    }
}

fn labels_to_category(
    labels: &[crate::api::types::Label],
    body: Option<&str>,
) -> (TicketCategory, Vec<String>, Option<String>) {
    let has_rfe = labels.iter().any(|l| l.name.eq_ignore_ascii_case("rfe"));
    let is_known_issue = labels
        .iter()
        .any(|l| l.name.eq_ignore_ascii_case("knownissue"));

    let category = if is_known_issue {
        TicketCategory::KnownIssue
    } else if has_rfe {
        TicketCategory::Enhancement
    } else {
        TicketCategory::BugFix
    };

    let changelog = scan_body_for_field(body, "changelog");
    let rhbz = scan_body_for_field(body, "rhbz").into_iter().next();

    (category, changelog, rhbz)
}

fn github_issue_to_release_ticket(issue: &crate::api::github::GitHubIssue) -> ReleaseTicket {
    let (category, changelog, rhbz) = labels_to_category(&issue.labels, issue.body.as_deref());
    ReleaseTicket {
        number: issue.number,
        title: issue.title.clone(),
        category,
        changelog,
        rhbz,
    }
}

fn forgejo_issue_to_release_ticket(issue: &crate::api::forgejo::ForgejoIssue) -> ReleaseTicket {
    let (category, changelog, rhbz) = labels_to_category(&issue.labels, issue.body.as_deref());
    ReleaseTicket {
        number: issue.number,
        title: issue.title.clone(),
        category,
        changelog,
        rhbz,
    }
}

fn fetch_single_ticket(ctx: &Ctx, number: u64) -> Result<ReleaseTicket> {
    match ctx.issue_tracker {
        IssueTracker::Pagure => {
            let pagure = ctx
                .pagure
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Pagure client not configured"))?;
            let issue = pagure.get_issue(number)?;
            Ok(pagure_issue_to_release_ticket(&issue))
        }
        IssueTracker::GitHub => {
            let github = ctx
                .github
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("GitHub client not configured"))?;
            let issue = github.get_issue(number)?;
            Ok(github_issue_to_release_ticket(&issue))
        }
        IssueTracker::Forgejo => {
            let forgejo = ctx
                .forgejo
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Forgejo client not configured"))?;
            let issue = forgejo.get_issue(number)?;
            Ok(forgejo_issue_to_release_ticket(&issue))
        }
    }
}

fn scan_body_for_field(body: Option<&str>, field_name: &str) -> Vec<String> {
    let Some(body) = body else {
        return Vec::new();
    };
    let needle = format!("{}:", field_name);
    let mut values = Vec::new();
    for line in body.lines() {
        if let Some(prefix) = line.get(..needle.len()) {
            if prefix.eq_ignore_ascii_case(&needle) {
                let rest = &line[needle.len()..];
                let v = rest.trim();
                if !v.is_empty() {
                    values.push(v.to_string());
                }
            }
        }
    }
    values
}

// ── Release notes categorization helpers ─────────────────────────────────────

fn release_notes_and_categories(
    tickets: &[ReleaseTicket],
    bullet: &str,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let indent: String = " ".repeat(bullet.len());
    let mut release_notes = Vec::new();
    let mut enhancements = Vec::new();
    let mut known_issues = Vec::new();

    for ticket in tickets {
        let changelog_text = if ticket.changelog.is_empty() {
            if ticket.title.contains("[RFE]") {
                String::new()
            } else {
                continue;
            }
        } else {
            ticket.changelog.join(" ")
        };

        let note = if changelog_text.is_empty() {
            format!("{}#{}: {}", bullet, ticket.number, ticket.title)
        } else {
            format!(
                "{}#{}: {}\n{}{}",
                bullet, ticket.number, ticket.title, indent, changelog_text
            )
        };

        match ticket.category {
            TicketCategory::KnownIssue => known_issues.push(note),
            TicketCategory::Enhancement => {
                enhancements.push(note.clone());
                release_notes.push(note);
            }
            TicketCategory::BugFix => release_notes.push(note),
        }
    }

    (release_notes, enhancements, known_issues)
}

fn approximate_bug_count(bugs: &[&ReleaseTicket]) -> String {
    let count = bugs.len();
    if count == 0 {
        return "no".to_string();
    }
    let remainder = count % 10;
    if remainder == 0 || count < 10 {
        format!("{}", count)
    } else {
        format!("more than {}", count - remainder)
    }
}

// ── Markdown output ──────────────────────────────────────────────────────────

fn print_markdown(
    tickets: &[ReleaseTicket],
    bugs: &[&ReleaseTicket],
    git: &GitLogResult,
    params: &ReleaseNotesParams<'_>,
    fmt: &FormatConfig<'_>,
) {
    let (release_notes, enhancements, known_issues) = release_notes_and_categories(tickets, "* ");

    println!("# FreeIPA {} Release Notes", params.version);
    println!();
    println!("**Release date**: {}", params.release_date);
    println!();
    println!(
        "The FreeIPA team would like to announce FreeIPA {} release!",
        params.version
    );
    println!();
    println!("It can be downloaded from http://www.freeipa.org/page/Downloads. Builds for");
    println!("Fedora distributions will be available from the official repository soon.");
    println!();

    println!("## Highlights in {}", params.version);
    println!();
    if !release_notes.is_empty() {
        println!("<!-- TODO: put release notes to proper categories -->");
        for note in &release_notes {
            println!("{}", note);
        }
        println!("<!-- END TODO -->");
    }
    println!();

    println!("## Enhancements");
    println!();
    if enhancements.is_empty() {
        println!("*(none)*");
    } else {
        for note in &enhancements {
            println!("{}", note);
        }
    }
    println!();

    println!("## Known Issues");
    println!();
    if known_issues.is_empty() {
        println!("*(none)*");
    } else {
        for note in &known_issues {
            println!("{}", note);
        }
    }
    println!();

    println!("## Bug Fixes");
    println!();
    println!(
        "FreeIPA {} is a stabilization release for the features delivered as a\n\
         part of {} version series.",
        params.version, params.major_version
    );
    println!();
    println!(
        "There are {} bug-fixes since FreeIPA {} release.\n\
         Details of the bug-fixes can be seen in the list of resolved tickets below.",
        approximate_bug_count(bugs),
        params.prev_version
    );
    println!();

    println!("## Upgrading");
    println!();
    println!("Upgrade instructions are available on the [Upgrade](https://www.freeipa.org/page/Upgrade) page.");
    println!();

    println!("## Feedback");
    println!();
    println!("Please provide comments, bugs and other feedback via the freeipa-users mailing");
    println!("list (https://lists.fedoraproject.org/archives/list/freeipa-users@lists.fedorahosted.org/)");
    println!("or #freeipa channel on libera.chat.");
    println!();

    println!("## Resolved Tickets");
    println!();
    for ticket in tickets {
        print_ticket_markdown(ticket, fmt);
    }
    println!();

    println!("## Detailed Changelog since {}", params.prev_version);
    println!();
    print_changelog_markdown(git, fmt);
}

fn print_ticket_markdown(ticket: &ReleaseTicket, fmt: &FormatConfig<'_>) {
    if fmt.links && !fmt.ticket_url.is_empty() {
        let mut line = format!(
            "* [#{}]({}{}) {}",
            ticket.number, fmt.ticket_url, ticket.number, ticket.title
        );
        if let Some(ref rhbz) = ticket.rhbz {
            let bz_links = format_rhbz_links_markdown(rhbz, fmt.bugzilla_bug_url);
            if !bz_links.is_empty() {
                line = format!("{} ({})", line, bz_links);
            }
        }
        println!("{}", line);
    } else {
        println!("* #{} {}", ticket.number, ticket.title);
    }
}

fn format_rhbz_links_markdown(rhbz: &str, bugzilla_bug_url: &str) -> String {
    let mut links = Vec::new();
    for part in rhbz.split(',') {
        let part = part.trim();
        if let Some(caps) = rhbz_re().captures(part) {
            if let Some(id) = caps.get(1) {
                links.push(format!("[rhbz#{}]({})", id.as_str(), part));
                continue;
            }
        }
        if !part.is_empty() && !bugzilla_bug_url.is_empty() && part.starts_with(bugzilla_bug_url) {
            let id = part.trim_start_matches(bugzilla_bug_url);
            links.push(format!("[rhbz#{}]({})", id, part));
        }
    }
    links.join(", ")
}

fn print_changelog_markdown(git: &GitLogResult, fmt: &FormatConfig<'_>) {
    for author in git.authors.values() {
        if author.commit_indices.is_empty() {
            continue;
        }
        println!("### {} ({})", author.name, author.commit_indices.len());
        println!();
        for &idx in &author.commit_indices {
            let commit = &git.commits[idx];
            print_commit_markdown(commit, fmt);
        }
        println!();
    }
}

fn print_commit_markdown(commit: &git_log::GitCommit, fmt: &FormatConfig<'_>) {
    let mut line = format!("* {}", commit.summary.trim());
    if fmt.links {
        if !fmt.commit_url.is_empty() {
            line = format!("{} [commit]({}{})", line, fmt.commit_url, commit.hash);
        }
        if !fmt.ticket_url.is_empty() {
            let ticket_links: Vec<String> = commit
                .tickets
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|t| format!("[#{}]({}{})", t, fmt.ticket_url, t))
                .collect();
            if !ticket_links.is_empty() {
                line = format!("{} {}", line, ticket_links.join(" "));
            }
        }
    }
    println!("{}", line);
}

// ── MediaWiki output ─────────────────────────────────────────────────────────

fn print_wiki(
    tickets: &[ReleaseTicket],
    bugs: &[&ReleaseTicket],
    git: &GitLogResult,
    params: &ReleaseNotesParams<'_>,
    fmt: &FormatConfig<'_>,
) {
    let (release_notes, enhancements, known_issues) = release_notes_and_categories(tickets, "* ");

    println!("{{{{ReleaseDate|{}}}}}", params.release_date);
    println!(
        "The FreeIPA team would like to announce FreeIPA {} release!",
        params.version
    );
    println!();
    println!("It can be downloaded from http://www.freeipa.org/page/Downloads. Builds for");
    println!("Fedora distributions will be available from the official repository soon.");
    println!();

    println!("== Highlights in {} ==", params.version);
    println!();
    if !release_notes.is_empty() {
        println!("'''TODO RELEASE NOTES - put release notes (if any) to proper categories'''");
        for note in &release_notes {
            println!("{}", note);
        }
        println!("'''END TODO'''");
    }
    println!();

    println!("=== Enhancements ===");
    for note in &enhancements {
        println!("{}", note);
    }
    println!();

    println!("=== Known Issues ===");
    for note in &known_issues {
        println!("{}", note);
    }
    println!();

    println!("=== Bug fixes ===");
    println!(
        "FreeIPA {} is a stabilization release for the features delivered as a\n\
         part of {} version series.",
        params.version, params.major_version
    );
    println!();
    println!(
        "There are {} bug-fixes since FreeIPA {} release.\n\
         Details of the bug-fixes can be seen in the list of resolved tickets below.",
        approximate_bug_count(bugs),
        params.prev_version
    );
    println!();

    println!("== Upgrading ==");
    println!(
        "Upgrade instructions are available on [https://www.freeipa.org/page/Upgrade Upgrade] page."
    );
    println!();

    println!("== Feedback ==");
    println!("Please provide comments, bugs and other feedback via the freeipa-users mailing");
    println!("list (https://lists.fedoraproject.org/archives/list/freeipa-users@lists.fedorahosted.org/)");
    println!("or #freeipa channel on libera.chat.");
    println!();

    println!("== Resolved tickets ==");
    for ticket in tickets {
        print_ticket_wiki(ticket, fmt);
    }
    println!();

    println!("== Detailed changelog since {} ==", params.prev_version);
    print_changelog_wiki(git, fmt);
}

fn print_ticket_wiki(ticket: &ReleaseTicket, fmt: &FormatConfig<'_>) {
    if fmt.links && !fmt.ticket_url.is_empty() {
        let mut line = format!(
            "* [{}{} #{}] {}",
            fmt.ticket_url, ticket.number, ticket.number, ticket.title
        );
        if let Some(ref rhbz) = ticket.rhbz {
            let bz_links = format_rhbz_links_wiki(rhbz, fmt.bugzilla_bug_url);
            if !bz_links.is_empty() {
                line = format!("{} ({})", line, bz_links);
            }
        }
        println!("{}", line);
    } else {
        println!("* {} {}", ticket.number, ticket.title);
    }
}

fn format_rhbz_links_wiki(rhbz: &str, bugzilla_bug_url: &str) -> String {
    let mut links = Vec::new();
    for part in rhbz.split(',') {
        let part = part.trim();
        if let Some(caps) = rhbz_re().captures(part) {
            if let Some(id) = caps.get(1) {
                links.push(format!("[{} rhbz#{}]", part, id.as_str()));
                continue;
            }
        }
        if !part.is_empty() && !bugzilla_bug_url.is_empty() && part.starts_with(bugzilla_bug_url) {
            let id = part.trim_start_matches(bugzilla_bug_url);
            links.push(format!("[{} rhbz#{}]", part, id));
        }
    }
    links.join(", ")
}

fn print_changelog_wiki(git: &GitLogResult, fmt: &FormatConfig<'_>) {
    for author in git.authors.values() {
        if author.commit_indices.is_empty() {
            continue;
        }
        println!("=== {} ({}) ===", author.name, author.commit_indices.len());
        for &idx in &author.commit_indices {
            let commit = &git.commits[idx];
            print_commit_wiki(commit, fmt);
        }
        println!();
    }
}

fn print_commit_wiki(commit: &git_log::GitCommit, fmt: &FormatConfig<'_>) {
    let mut line = format!("* {}", commit.summary.trim());
    if fmt.links {
        if !fmt.commit_url.is_empty() {
            line = format!("{} [{}{} commit]", line, fmt.commit_url, commit.hash);
        }
        if !fmt.ticket_url.is_empty() {
            let ticket_links: Vec<String> = commit
                .tickets
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|t| format!("[{}{} #{}]", fmt.ticket_url, t, t))
                .collect();
            if !ticket_links.is_empty() {
                line = format!("{} {}", line, ticket_links.join(" "));
            }
        }
    }
    println!("{}", line);
}

// ── reStructuredText output ──────────────────────────────────────────────────

fn rst_title(text: &str) {
    let underline = "=".repeat(text.len());
    println!("{}", underline);
    println!("{}", text);
    println!("{}", underline);
}

fn rst_heading(text: &str, ch: char) {
    let underline: String = std::iter::repeat_n(ch, text.len()).collect();
    println!("{}", text);
    println!("{}", underline);
}

fn print_rst(
    tickets: &[ReleaseTicket],
    bugs: &[&ReleaseTicket],
    git: &GitLogResult,
    params: &ReleaseNotesParams<'_>,
    fmt: &FormatConfig<'_>,
) {
    let (release_notes, enhancements, known_issues) = release_notes_and_categories(tickets, "* ");

    let title = format!("FreeIPA {} Release Notes", params.version);
    rst_title(&title);
    println!();
    println!("**Release date**: {}", params.release_date);
    println!();
    println!(
        "The FreeIPA team would like to announce FreeIPA {} release!",
        params.version
    );
    println!();
    println!("It can be downloaded from http://www.freeipa.org/page/Downloads. Builds for");
    println!("Fedora distributions will be available from the official repository soon.");
    println!();

    let heading = format!("Highlights in {}", params.version);
    rst_heading(&heading, '-');
    println!();
    if !release_notes.is_empty() {
        println!(".. TODO:: put release notes to proper categories");
        println!();
        for note in &release_notes {
            println!("{}", note);
        }
        println!();
    }

    rst_heading("Enhancements", '-');
    println!();
    if enhancements.is_empty() {
        println!("*none*");
    } else {
        for note in &enhancements {
            println!("{}", note);
        }
    }
    println!();

    rst_heading("Known Issues", '-');
    println!();
    if known_issues.is_empty() {
        println!("*none*");
    } else {
        for note in &known_issues {
            println!("{}", note);
        }
    }
    println!();

    rst_heading("Bug Fixes", '-');
    println!();
    println!(
        "FreeIPA {} is a stabilization release for the features delivered as a\n\
         part of {} version series.",
        params.version, params.major_version
    );
    println!();
    println!(
        "There are {} bug-fixes since FreeIPA {} release.\n\
         Details of the bug-fixes can be seen in the list of resolved tickets below.",
        approximate_bug_count(bugs),
        params.prev_version
    );
    println!();

    rst_heading("Upgrading", '-');
    println!();
    println!(
        "Upgrade instructions are available on the `Upgrade <https://www.freeipa.org/page/Upgrade>`__ page."
    );
    println!();

    rst_heading("Feedback", '-');
    println!();
    println!("Please provide comments, bugs and other feedback via the freeipa-users mailing");
    println!("list (https://lists.fedoraproject.org/archives/list/freeipa-users@lists.fedorahosted.org/)");
    println!("or #freeipa channel on libera.chat.");
    println!();

    rst_heading("Resolved Tickets", '-');
    println!();
    for ticket in tickets {
        print_ticket_rst(ticket, fmt);
    }
    println!();

    let heading = format!("Detailed Changelog since {}", params.prev_version);
    rst_heading(&heading, '-');
    println!();
    print_changelog_rst(git, fmt);
}

fn print_ticket_rst(ticket: &ReleaseTicket, fmt: &FormatConfig<'_>) {
    if fmt.links && !fmt.ticket_url.is_empty() {
        let mut line = format!(
            "* `#{} <{}{}>`__ {}",
            ticket.number, fmt.ticket_url, ticket.number, ticket.title
        );
        if let Some(ref rhbz) = ticket.rhbz {
            let bz_links = format_rhbz_links_rst(rhbz, fmt.bugzilla_bug_url);
            if !bz_links.is_empty() {
                line = format!("{} ({})", line, bz_links);
            }
        }
        println!("{}", line);
    } else {
        println!("* #{} {}", ticket.number, ticket.title);
    }
}

fn format_rhbz_links_rst(rhbz: &str, bugzilla_bug_url: &str) -> String {
    let mut links = Vec::new();
    for part in rhbz.split(',') {
        let part = part.trim();
        if let Some(caps) = rhbz_re().captures(part) {
            if let Some(id) = caps.get(1) {
                links.push(format!("`rhbz#{} <{}>`__", id.as_str(), part));
                continue;
            }
        }
        if !part.is_empty() && !bugzilla_bug_url.is_empty() && part.starts_with(bugzilla_bug_url) {
            let id = part.trim_start_matches(bugzilla_bug_url);
            links.push(format!("`rhbz#{} <{}>`__", id, part));
        }
    }
    links.join(", ")
}

fn print_changelog_rst(git: &GitLogResult, fmt: &FormatConfig<'_>) {
    for author in git.authors.values() {
        if author.commit_indices.is_empty() {
            continue;
        }
        let heading = format!("{} ({})", author.name, author.commit_indices.len());
        rst_heading(&heading, '~');
        println!();
        for &idx in &author.commit_indices {
            let commit = &git.commits[idx];
            print_commit_rst(commit, fmt);
        }
        println!();
    }
}

fn print_commit_rst(commit: &git_log::GitCommit, fmt: &FormatConfig<'_>) {
    let mut line = format!("* {}", commit.summary.trim());
    if fmt.links {
        if !fmt.commit_url.is_empty() {
            line = format!("{} `commit <{}{}>`__", line, fmt.commit_url, commit.hash);
        }
        if !fmt.ticket_url.is_empty() {
            let ticket_links: Vec<String> = commit
                .tickets
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|t| format!("`#{} <{}{}>`__", t, fmt.ticket_url, t))
                .collect();
            if !ticket_links.is_empty() {
                line = format!("{} {}", line, ticket_links.join(" "));
            }
        }
    }
    println!("{}", line);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::Label;

    // ── scan_body_for_field ──────────────────────────────────────────────────

    #[test]
    fn test_scan_body_for_field_normal_case() {
        let body = "changelog: Fixed the widget\nrhbz: https://bugzilla.redhat.com/123";
        let result = scan_body_for_field(Some(body), "changelog");
        assert_eq!(result, vec!["Fixed the widget"]);
    }

    #[test]
    fn test_scan_body_for_field_case_insensitive() {
        let body = "Changelog: Fixed the widget";
        let result = scan_body_for_field(Some(body), "changelog");
        assert_eq!(result, vec!["Fixed the widget"]);

        let body2 = "CHANGELOG: Another fix";
        let result2 = scan_body_for_field(Some(body2), "changelog");
        assert_eq!(result2, vec!["Another fix"]);
    }

    #[test]
    fn test_scan_body_for_field_empty_body() {
        let result = scan_body_for_field(None, "changelog");
        assert!(result.is_empty());
    }

    #[test]
    fn test_scan_body_for_field_no_match() {
        let body = "This is a description\nNo fields here";
        let result = scan_body_for_field(Some(body), "changelog");
        assert!(result.is_empty());
    }

    #[test]
    fn test_scan_body_for_field_multi_line() {
        let body = "changelog: First entry\nother stuff\nchangelog: Second entry";
        let result = scan_body_for_field(Some(body), "changelog");
        assert_eq!(result, vec!["First entry", "Second entry"]);
    }

    // ── approximate_bug_count ───────────────────────────────────────────────

    #[test]
    fn test_approximate_bug_count_zero() {
        let bugs: Vec<&ReleaseTicket> = vec![];
        assert_eq!(approximate_bug_count(&bugs), "no");
    }

    #[test]
    fn test_approximate_bug_count_one() {
        let t = ReleaseTicket {
            number: 1,
            title: "bug".to_string(),
            category: TicketCategory::BugFix,
            changelog: vec![],
            rhbz: None,
        };
        let bugs: Vec<&ReleaseTicket> = vec![&t];
        assert_eq!(approximate_bug_count(&bugs), "1");
    }

    #[test]
    fn test_approximate_bug_count_nine() {
        let tickets: Vec<ReleaseTicket> = (0..9)
            .map(|i| ReleaseTicket {
                number: i,
                title: "bug".to_string(),
                category: TicketCategory::BugFix,
                changelog: vec![],
                rhbz: None,
            })
            .collect();
        let bugs: Vec<&ReleaseTicket> = tickets.iter().collect();
        assert_eq!(approximate_bug_count(&bugs), "9");
    }

    #[test]
    fn test_approximate_bug_count_ten() {
        let tickets: Vec<ReleaseTicket> = (0..10)
            .map(|i| ReleaseTicket {
                number: i,
                title: "bug".to_string(),
                category: TicketCategory::BugFix,
                changelog: vec![],
                rhbz: None,
            })
            .collect();
        let bugs: Vec<&ReleaseTicket> = tickets.iter().collect();
        assert_eq!(approximate_bug_count(&bugs), "10");
    }

    #[test]
    fn test_approximate_bug_count_eleven() {
        let tickets: Vec<ReleaseTicket> = (0..11)
            .map(|i| ReleaseTicket {
                number: i,
                title: "bug".to_string(),
                category: TicketCategory::BugFix,
                changelog: vec![],
                rhbz: None,
            })
            .collect();
        let bugs: Vec<&ReleaseTicket> = tickets.iter().collect();
        assert_eq!(approximate_bug_count(&bugs), "more than 10");
    }

    #[test]
    fn test_approximate_bug_count_ninety_nine() {
        let tickets: Vec<ReleaseTicket> = (0..99)
            .map(|i| ReleaseTicket {
                number: i,
                title: "bug".to_string(),
                category: TicketCategory::BugFix,
                changelog: vec![],
                rhbz: None,
            })
            .collect();
        let bugs: Vec<&ReleaseTicket> = tickets.iter().collect();
        assert_eq!(approximate_bug_count(&bugs), "more than 90");
    }

    #[test]
    fn test_approximate_bug_count_hundred() {
        let tickets: Vec<ReleaseTicket> = (0..100)
            .map(|i| ReleaseTicket {
                number: i,
                title: "bug".to_string(),
                category: TicketCategory::BugFix,
                changelog: vec![],
                rhbz: None,
            })
            .collect();
        let bugs: Vec<&ReleaseTicket> = tickets.iter().collect();
        assert_eq!(approximate_bug_count(&bugs), "100");
    }

    // ── format_rhbz_links_markdown ──────────────────────────────────────────

    #[test]
    fn test_rhbz_markdown_full_url() {
        let result =
            format_rhbz_links_markdown("https://bugzilla.redhat.com/show_bug.cgi?id=12345", "");
        assert_eq!(
            result,
            "[rhbz#12345](https://bugzilla.redhat.com/show_bug.cgi?id=12345)"
        );
    }

    #[test]
    fn test_rhbz_markdown_bugzilla_bug_url_match() {
        let result =
            format_rhbz_links_markdown("https://bz.example.com/67890", "https://bz.example.com/");
        assert_eq!(result, "[rhbz#67890](https://bz.example.com/67890)");
    }

    #[test]
    fn test_rhbz_markdown_empty() {
        let result = format_rhbz_links_markdown("", "");
        assert_eq!(result, "");
    }

    #[test]
    fn test_rhbz_markdown_no_match() {
        let result = format_rhbz_links_markdown("not a url", "https://bz.example.com/");
        assert_eq!(result, "");
    }

    // ── format_rhbz_links_wiki ──────────────────────────────────────────────

    #[test]
    fn test_rhbz_wiki_full_url() {
        let result =
            format_rhbz_links_wiki("https://bugzilla.redhat.com/show_bug.cgi?id=12345", "");
        assert_eq!(
            result,
            "[https://bugzilla.redhat.com/show_bug.cgi?id=12345 rhbz#12345]"
        );
    }

    #[test]
    fn test_rhbz_wiki_bugzilla_bug_url_match() {
        let result =
            format_rhbz_links_wiki("https://bz.example.com/67890", "https://bz.example.com/");
        assert_eq!(result, "[https://bz.example.com/67890 rhbz#67890]");
    }

    #[test]
    fn test_rhbz_wiki_empty() {
        let result = format_rhbz_links_wiki("", "");
        assert_eq!(result, "");
    }

    #[test]
    fn test_rhbz_wiki_no_match() {
        let result = format_rhbz_links_wiki("not a url", "https://bz.example.com/");
        assert_eq!(result, "");
    }

    // ── release_notes_and_categories ────────────────────────────────────────

    #[test]
    fn test_categorization_bug_fix() {
        let tickets = vec![ReleaseTicket {
            number: 1,
            title: "Fix crash".to_string(),
            category: TicketCategory::BugFix,
            changelog: vec!["Fixed a crash".to_string()],
            rhbz: None,
        }];
        let (notes, enhancements, known) = release_notes_and_categories(&tickets, "* ");
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("Fix crash"));
        assert!(enhancements.is_empty());
        assert!(known.is_empty());
    }

    #[test]
    fn test_categorization_enhancement() {
        let tickets = vec![ReleaseTicket {
            number: 2,
            title: "[RFE] Add widget".to_string(),
            category: TicketCategory::Enhancement,
            changelog: vec!["Added widget support".to_string()],
            rhbz: None,
        }];
        let (notes, enhancements, known) = release_notes_and_categories(&tickets, "* ");
        assert_eq!(notes.len(), 1);
        assert_eq!(enhancements.len(), 1);
        assert!(known.is_empty());
    }

    #[test]
    fn test_categorization_known_issue() {
        let tickets = vec![ReleaseTicket {
            number: 3,
            title: "Known problem".to_string(),
            category: TicketCategory::KnownIssue,
            changelog: vec!["This is known".to_string()],
            rhbz: None,
        }];
        let (notes, _enhancements, known) = release_notes_and_categories(&tickets, "* ");
        assert!(notes.is_empty());
        assert_eq!(known.len(), 1);
    }

    #[test]
    fn test_rfe_empty_changelog_no_trailing_whitespace() {
        let tickets = vec![ReleaseTicket {
            number: 42,
            title: "[RFE] New feature".to_string(),
            category: TicketCategory::Enhancement,
            changelog: vec![],
            rhbz: None,
        }];
        let (notes, enhancements, _known) = release_notes_and_categories(&tickets, "* ");
        assert_eq!(notes.len(), 1);
        assert_eq!(enhancements.len(), 1);
        // Must not have trailing whitespace
        for line in notes[0].lines() {
            assert_eq!(
                line,
                line.trim_end(),
                "trailing whitespace found in: {:?}",
                line
            );
        }
    }

    // ── labels_to_category ──────────────────────────────────────────────────

    fn make_label(name: &str) -> Label {
        Label {
            name: name.to_string(),
            color: "000000".to_string(),
        }
    }

    #[test]
    fn test_labels_to_category_rfe() {
        let labels = vec![make_label("rfe")];
        let (cat, _, _) = labels_to_category(&labels, None);
        assert_eq!(cat, TicketCategory::Enhancement);
    }

    #[test]
    fn test_labels_to_category_knownissue() {
        let labels = vec![make_label("knownissue")];
        let (cat, _, _) = labels_to_category(&labels, None);
        assert_eq!(cat, TicketCategory::KnownIssue);
    }

    #[test]
    fn test_labels_to_category_no_label() {
        let labels: Vec<Label> = vec![];
        let (cat, _, _) = labels_to_category(&labels, None);
        assert_eq!(cat, TicketCategory::BugFix);
    }

    #[test]
    fn test_labels_to_category_both_labels() {
        let labels = vec![make_label("rfe"), make_label("knownissue")];
        let (cat, _, _) = labels_to_category(&labels, None);
        // knownissue takes priority
        assert_eq!(cat, TicketCategory::KnownIssue);
    }

    #[test]
    fn test_labels_to_category_with_body() {
        let labels = vec![make_label("rfe")];
        let body =
            "changelog: Added new feature\nrhbz: https://bugzilla.redhat.com/show_bug.cgi?id=999";
        let (cat, changelog, rhbz) = labels_to_category(&labels, Some(body));
        assert_eq!(cat, TicketCategory::Enhancement);
        assert_eq!(changelog, vec!["Added new feature"]);
        assert_eq!(
            rhbz,
            Some("https://bugzilla.redhat.com/show_bug.cgi?id=999".to_string())
        );
    }

    // ── format_rhbz_links_rst ──────────────────────────────────────────────

    #[test]
    fn test_rhbz_rst_full_url() {
        let result =
            format_rhbz_links_rst("https://bugzilla.redhat.com/show_bug.cgi?id=12345", "");
        assert_eq!(
            result,
            "`rhbz#12345 <https://bugzilla.redhat.com/show_bug.cgi?id=12345>`__"
        );
    }

    #[test]
    fn test_rhbz_rst_bugzilla_bug_url_match() {
        let result =
            format_rhbz_links_rst("https://bz.example.com/67890", "https://bz.example.com/");
        assert_eq!(result, "`rhbz#67890 <https://bz.example.com/67890>`__");
    }

    #[test]
    fn test_rhbz_rst_empty() {
        let result = format_rhbz_links_rst("", "");
        assert_eq!(result, "");
    }

    #[test]
    fn test_rhbz_rst_no_match() {
        let result = format_rhbz_links_rst("not a url", "https://bz.example.com/");
        assert_eq!(result, "");
    }
}
