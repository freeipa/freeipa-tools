//! Clone issues across trackers: create a copy of an issue (title + body +
//! selected comments) in another tracker (Pagure, Forgejo, GitHub, Bugzilla, Jira).

use anyhow::{bail, Result};
use std::sync::Arc;

use crate::api::{
    bugzilla::BugzillaClient, forgejo::ForgejoClient, github::GitHubClient, jira::JiraClient,
    pagure::PagureClient,
};
use crate::config::{Config, IssueTracker};
use crate::output::{prompt, ColorMode, Output};

/// Everything the clone flow needs.  `Ctx` is not cloneable (it owns an
/// `Output`), and the TUI needs an owned snapshot it can hand to worker
/// threads, so the clients are collected here as cheap `Arc` clones.
#[derive(Clone)]
pub struct CloneEnv {
    pub config: Config,
    pub pagure: Option<Arc<PagureClient>>,
    pub forgejo: Option<Arc<ForgejoClient>>,
    pub github: Option<Arc<GitHubClient>>,
    pub bugzilla: Option<Arc<BugzillaClient>>,
    pub jira: Option<Arc<JiraClient>>,
    pub issue_tracker: IssueTracker,
    pub color: String,
    pub dry_run: bool,
}

impl CloneEnv {
    pub fn from_ctx(ctx: &super::Ctx) -> CloneEnv {
        CloneEnv {
            config: ctx.config.clone(),
            pagure: ctx.pagure.clone(),
            forgejo: ctx.forgejo.clone(),
            github: ctx.github.clone(),
            bugzilla: ctx.bugzilla.clone(),
            jira: ctx.jira.clone(),
            issue_tracker: ctx.issue_tracker.clone(),
            color: ctx.color.clone(),
            dry_run: ctx.dry_run,
        }
    }

    fn out(&self) -> Output {
        Output::new(ColorMode::from_str(&self.color))
    }
}

/// A parsed issue reference: "bugzilla:1234", "jira:RHEL-99", a URL matching a
/// configured URL template, or a bare number (the configured issue tracker).
#[derive(Debug, Clone, PartialEq)]
pub struct IssueRef {
    pub kind: Kind,
    pub id: String,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Kind {
    Pagure,
    Forgejo,
    GitHub,
    Bugzilla,
    Jira,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s.to_ascii_lowercase().as_str() {
            "pagure" => Kind::Pagure,
            "forgejo" | "codeberg" => Kind::Forgejo,
            "github" | "gh" => Kind::GitHub,
            "bugzilla" | "bz" | "rhbz" => Kind::Bugzilla,
            "jira" => Kind::Jira,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Pagure => "Pagure",
            Kind::Forgejo => "Forgejo",
            Kind::GitHub => "GitHub",
            Kind::Bugzilla => "Bugzilla",
            Kind::Jira => "Jira",
        }
    }

    /// Lowercase keyword accepted by `--to` and the `tracker:id` form.
    pub fn keyword(self) -> &'static str {
        match self {
            Kind::Pagure => "pagure",
            Kind::Forgejo => "forgejo",
            Kind::GitHub => "github",
            Kind::Bugzilla => "bugzilla",
            Kind::Jira => "jira",
        }
    }
}

/// Content of a fetched source issue.
struct SourceContent {
    title: String,
    body: String,
    comments: Vec<String>,
    origin_url: String,
}

pub fn parse_ref(env: &CloneEnv, s: &str) -> Result<IssueRef> {
    let s = s.trim();
    if s.is_empty() {
        bail!("empty issue reference");
    }

    // URL form: classify by the configured URL templates.
    if s.contains("://") {
        let mut derived: Vec<(Kind, String)> = Vec::new();
        if env.config.has_forgejo() {
            if let Some(u) = forgejo_issue_url(env) {
                derived.push((Kind::Forgejo, u));
            }
        }
        if env.config.has_github() {
            derived.push((
                Kind::GitHub,
                format!("https://github.com/{}/issues/", env.config.gh_repo),
            ));
        }
        let bases = [
            (Kind::Pagure, env.config.ticket_url.clone()),
            (Kind::Bugzilla, env.config.bugzilla_bug_url.clone()),
            (Kind::Jira, jira_prefix(env)),
        ];
        for (kind, base) in bases.iter().chain(derived.iter()) {
            if base.is_empty() || !s.starts_with(base.as_str()) {
                continue;
            }
            let id = s[base.len()..].trim_matches('/');
            if !id.is_empty() {
                return Ok(IssueRef {
                    kind: *kind,
                    id: id.to_string(),
                });
            }
        }
        bail!(
            "cannot classify issue URL {s}; configure ticket-url/bugzilla-bug-url/jira-ticket-url \
             or use the <tracker>:<id> form (pagure|forgejo|github|bugzilla|jira)"
        );
    }

    // Prefixed form: tracker:id
    if let Some((prefix, id)) = s.split_once(':') {
        if let Some(kind) = Kind::parse(prefix) {
            let id = id.trim();
            if id.is_empty() {
                bail!("missing issue id in {s}");
            }
            return Ok(IssueRef {
                kind,
                id: id.to_string(),
            });
        }
    }

    // Bare number: the configured issue tracker.
    if s.chars().all(|c| c.is_ascii_digit()) {
        let kind = match env.issue_tracker {
            IssueTracker::Pagure => Kind::Pagure,
            IssueTracker::Forgejo => Kind::Forgejo,
            IssueTracker::GitHub => Kind::GitHub,
        };
        return Ok(IssueRef {
            kind,
            id: s.to_string(),
        });
    }

    bail!("unrecognized issue reference: {s}");
}

fn forgejo_issue_url(env: &CloneEnv) -> Option<String> {
    let base = env.config.forgejo_url.trim_end_matches('/');
    Some(format!("{}/{}/issues/", base, env.config.forgejo_repo))
}

/// Jira ids are full issue keys (RHEL-99), matching how push/pr-push strip
/// `{server}/browse/` from ticket URLs.  Prefer that prefix over the raw
/// jira-ticket-url template, which includes the project key.
fn jira_prefix(env: &CloneEnv) -> String {
    match env.config.jira_server() {
        Some(server) => format!("{}/browse/", server),
        None => env.config.jira_ticket_url.clone(),
    }
}

/// Find issue references of any configured tracker inside free text (PR
/// bodies, commit messages).  Used to pre-fill the TUI clone dialog.
pub fn find_refs(env: &CloneEnv, text: &str) -> Vec<IssueRef> {
    let mut bases: Vec<(Kind, String)> = vec![
        (Kind::Pagure, env.config.ticket_url.clone()),
        (Kind::Bugzilla, env.config.bugzilla_bug_url.clone()),
        (Kind::Jira, jira_prefix(env)),
    ];
    if env.config.has_forgejo() {
        if let Some(u) = forgejo_issue_url(env) {
            bases.push((Kind::Forgejo, u));
        }
    }
    if env.config.has_github() {
        bases.push((
            Kind::GitHub,
            format!("https://github.com/{}/issues/", env.config.gh_repo),
        ));
    }

    let mut found = Vec::new();
    for token in text.split_whitespace() {
        let token =
            token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '/' && c != ':');
        for (kind, base) in &bases {
            if base.is_empty() || !token.starts_with(base.as_str()) {
                continue;
            }
            let id = token[base.len()..].trim_end_matches('/');
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                continue;
            }
            let r = IssueRef {
                kind: *kind,
                id: id.to_string(),
            };
            if !found.contains(&r) {
                found.push(r);
            }
        }
    }
    found
}

// ── Fetching the source ──────────────────────────────────────────────────────

fn err_not_configured(what: &str) -> anyhow::Error {
    anyhow::anyhow!("'{what}' is not configured")
}

fn fetch(env: &CloneEnv, r: &IssueRef) -> Result<SourceContent> {
    match r.kind {
        Kind::Pagure => {
            let client = env
                .pagure
                .as_ref()
                .ok_or_else(|| err_not_configured("pagure"))?;
            let id: u64 =
                r.id.parse()
                    .map_err(|_| anyhow::anyhow!("Pagure id must be numeric: {}", r.id))?;
            let issue = client.get_issue(id)?;
            Ok(SourceContent {
                title: issue.title.clone(),
                body: issue.content.clone(),
                comments: issue.comments.iter().map(|c| c.comment.clone()).collect(),
                origin_url: format!("{}{}", env.config.ticket_url, id),
            })
        }
        Kind::Forgejo => {
            let client = env
                .forgejo
                .as_ref()
                .ok_or_else(|| err_not_configured("forgejo"))?;
            let id: u64 =
                r.id.parse()
                    .map_err(|_| anyhow::anyhow!("Forgejo id must be numeric: {}", r.id))?;
            let issue = client.get_issue(id)?;
            let comments = client
                .get_all_issue_comments(id)?
                .into_iter()
                .map(|c| c.body)
                .collect();
            let origin = forgejo_issue_url(env)
                .map(|u| format!("{}{}", u, id))
                .unwrap_or_default();
            Ok(SourceContent {
                title: issue.title.clone(),
                body: issue.body.unwrap_or_default(),
                comments,
                origin_url: origin,
            })
        }
        Kind::GitHub => {
            let client = env
                .github
                .as_ref()
                .ok_or_else(|| err_not_configured("github"))?;
            let id: u64 =
                r.id.parse()
                    .map_err(|_| anyhow::anyhow!("GitHub id must be numeric: {}", r.id))?;
            let issue = client.get_issue(id)?;
            let comments = client
                .get_all_issue_comments(id)?
                .into_iter()
                .map(|c| c.body)
                .collect();
            Ok(SourceContent {
                title: issue.title.clone(),
                body: issue.body.unwrap_or_default(),
                comments,
                origin_url: format!("https://github.com/{}/issues/{}", env.config.gh_repo, id),
            })
        }
        Kind::Bugzilla => {
            let client = env
                .bugzilla
                .as_ref()
                .ok_or_else(|| err_not_configured("bugzilla-url/bugzilla-api-key"))?;
            let id: u64 =
                r.id.parse()
                    .map_err(|_| anyhow::anyhow!("Bugzilla id must be numeric: {}", r.id))?;
            let (title, body, comments) = client.get_bug(id)?;
            Ok(SourceContent {
                title,
                body,
                comments,
                origin_url: format!("{}{}", env.config.bugzilla_bug_url, id),
            })
        }
        Kind::Jira => {
            let client = env
                .jira
                .as_ref()
                .ok_or_else(|| err_not_configured("jira"))?;
            let (title, body, comments) = client.get_issue_text(&r.id)?;
            Ok(SourceContent {
                title,
                body,
                comments,
                origin_url: format!("{}{}", jira_prefix(env), r.id),
            })
        }
    }
}

// ── Creating the copy ────────────────────────────────────────────────────────

/// Build the body of the new issue: origin marker, source body, then the last
/// `comments` comments of the original.
fn build_body(src: &SourceContent, comments: usize) -> String {
    let mut body = format!("Cloned from: {}\n\n", src.origin_url);
    body.push_str(src.body.trim_end());
    let n = comments.min(src.comments.len());
    if n > 0 {
        body.push_str("\n\n---\nComments from the original issue:\n");
        let sep = "─".repeat(40);
        for c in src.comments.iter().rev().take(n).rev() {
            body.push_str(&format!("\n{}\n{}\n", sep, c.trim()));
        }
    }
    body
}

/// Reject a target the config cannot serve, with a message naming the keys.
pub fn check_target(env: &CloneEnv, to: Kind) -> Result<()> {
    match to {
        Kind::Pagure if env.pagure.is_none() => {
            bail!("no Pagure project configured (pagure-token/pagure-repository)")
        }
        Kind::Forgejo if env.forgejo.is_none() => {
            bail!("no Forgejo project configured (forgejo-url/forgejo-repo/forgejo-token)")
        }
        Kind::GitHub if env.github.is_none() => {
            bail!("no GitHub repo configured (gh-token/gh-repo)")
        }
        Kind::Bugzilla if env.bugzilla.is_none() => {
            bail!("no Bugzilla configured (bugzilla-url/bugzilla-api-key/bugzilla-product)")
        }
        Kind::Jira if env.jira.is_none() => {
            bail!(
                "no Jira configured (jira-token; jira-ticket-url must contain /browse/<PROJECT>-)"
            )
        }
        Kind::Bugzilla if env.config.bugzilla_product.is_empty() => {
            bail!("bugzilla-product must be set to create bugs")
        }
        Kind::Jira if env.config.jira_project_key().is_none() => {
            bail!("cannot derive a Jira project key from jira-ticket-url")
        }
        _ => {}
    }
    Ok(())
}

/// Target trackers the current configuration can create issues in.
pub fn available_targets(env: &CloneEnv) -> Vec<Kind> {
    [
        Kind::Pagure,
        Kind::Forgejo,
        Kind::GitHub,
        Kind::Bugzilla,
        Kind::Jira,
    ]
    .into_iter()
    .filter(|k| check_target(env, *k).is_ok())
    .collect()
}

/// Pick the preferred default target for a clone (first usable, biased to
/// Jira then Bugzilla: that is the common upstream → downstream flow).
pub fn preferred_target(env: &CloneEnv) -> Option<Kind> {
    let targets = available_targets(env);
    for k in [Kind::Jira, Kind::Bugzilla] {
        if targets.contains(&k) {
            return Some(k);
        }
    }
    targets.into_iter().next()
}

/// Create the issue in the target tracker; returns a human-readable id.
pub fn create(env: &CloneEnv, kind: Kind, title: &str, body: &str) -> Result<String> {
    match kind {
        Kind::Pagure => env
            .pagure
            .as_ref()
            .expect("checked by check_target")
            .create_issue(title, body)
            .map(|id| format!("#{}", id)),
        Kind::Forgejo => env
            .forgejo
            .as_ref()
            .expect("checked by check_target")
            .create_issue(title, body)
            .map(|id| format!("#{}", id)),
        Kind::GitHub => env
            .github
            .as_ref()
            .expect("checked by check_target")
            .create_issue(title, body)
            .map(|id| format!("#{}", id)),
        Kind::Bugzilla => env
            .bugzilla
            .as_ref()
            .expect("checked by check_target")
            .create_bug(
                &env.config.bugzilla_product,
                &env.config.bugzilla_component,
                title,
                body,
            )
            .map(|id| format!("#{}", id)),
        Kind::Jira => env
            .jira
            .as_ref()
            .expect("checked by check_target")
            .create_issue(
                &env.config
                    .jira_project_key()
                    .expect("checked by check_target"),
                &env.config.jira_issuetype,
                title,
                body,
            ),
    }
}

pub struct CloneParams<'a> {
    pub source: &'a str,
    pub to: Kind,
    pub comments: usize,
    pub title: Option<&'a str>,
    /// Skip the interactive confirmation.
    pub yes: bool,
}

/// Fetch a source issue and create a copy of it in another tracker.
pub fn run(env: &CloneEnv, params: &CloneParams) -> Result<()> {
    let prepared = prepare(env, params)?;
    let out = env.out();

    out.section("Clone issue");
    println!(
        "{}:{}  →  {}",
        prepared.src_kind.label(),
        prepared.src_id,
        params.to.label()
    );
    println!("Title: {}", prepared.title);
    println!(
        "Body:  {} bytes, {} comment(s) included",
        prepared.body.len(),
        prepared.comments_copied
    );

    if env.dry_run {
        out.print_yellow("\n--dry-run: nothing was created");
        println!();
        println!("{}", prepared.body);
        return Ok(());
    }
    if !params.yes {
        let answer = prompt("Create the issue? [y/n] ");
        if !answer.eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let id = create(env, params.to, &prepared.title, &prepared.body)?;
    out.print_green(&format!(
        "Created {}{} (cloned from {} {})",
        params.to.label(),
        id,
        prepared.src_kind.label(),
        prepared.src_id
    ));
    Ok(())
}

/// Fetched + rendered copy, ready to be created.
pub struct Prepared {
    pub src_kind: Kind,
    pub src_id: String,
    pub title: String,
    pub body: String,
    pub comments_copied: usize,
}

/// Resolve + fetch + render without creating anything.  Shared by the CLI and
/// the TUI (which shows this in a confirmation dialog first).
pub fn prepare(env: &CloneEnv, params: &CloneParams) -> Result<Prepared> {
    let src_ref = parse_ref(env, params.source)?;
    check_target(env, params.to)?;
    if params.to == src_ref.kind {
        bail!("source and target trackers are both {}", params.to.label());
    }
    let src = fetch(env, &src_ref)?;
    let title = params.title.unwrap_or(&src.title).trim().to_string();
    if title.is_empty() {
        bail!("refusing to create an issue with an empty title");
    }
    let body = build_body(&src, params.comments);
    let comments_copied = params.comments.min(src.comments.len());
    Ok(Prepared {
        src_kind: src_ref.kind,
        src_id: src_ref.id,
        title,
        body,
        comments_copied,
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IssueTracker;

    fn env() -> CloneEnv {
        let config = Config {
            ticket_url: "https://pagure.example/freeipa/issues/".to_string(),
            bugzilla_bug_url: "https://bugzilla.example/show_bug.cgi?id=".to_string(),
            jira_ticket_url: "https://jira.example/browse/".to_string(),
            gh_repo: "freeipa/freeipa".to_string(),
            ..Config::default()
        };
        CloneEnv {
            config,
            pagure: None,
            forgejo: None,
            github: None,
            bugzilla: None,
            jira: None,
            issue_tracker: IssueTracker::Pagure,
            color: "never".to_string(),
            dry_run: false,
        }
    }

    #[test]
    fn parse_url_forms_classify_by_configured_base() {
        let e = env();
        assert_eq!(
            parse_ref(&e, "https://bugzilla.example/show_bug.cgi?id=1234").unwrap(),
            IssueRef {
                kind: Kind::Bugzilla,
                id: "1234".to_string()
            }
        );
        assert_eq!(
            parse_ref(&e, "https://jira.example/browse/RHEL-99").unwrap(),
            IssueRef {
                kind: Kind::Jira,
                id: "RHEL-99".to_string()
            }
        );
        // A configured jira-ticket-url normally embeds the project key
        // ("…/browse/RHEL-"); ids must still be the FULL key (RHEL-99),
        // matching how push/pr-push derive Jira keys from commit URLs.
        let mut e2 = env();
        e2.config.jira_ticket_url = "https://jira.example/browse/RHEL-".to_string();
        assert_eq!(
            parse_ref(&e2, "https://jira.example/browse/RHEL-99").unwrap(),
            IssueRef {
                kind: Kind::Jira,
                id: "RHEL-99".to_string()
            }
        );
        let found = find_refs(&e2, "Resolves: https://jira.example/browse/RHEL-99");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "RHEL-99");
        // Unclassifiable URL must fail with a message naming the alternatives.
        let err = parse_ref(&e, "https://other.example/issues/1")
            .unwrap_err()
            .to_string();
        assert!(err.contains("bugzilla-bug-url"), "{err}");
    }

    #[test]
    fn parse_prefixed_and_bare_forms() {
        let e = env();
        assert_eq!(
            parse_ref(&e, "bz:1234").unwrap(),
            IssueRef {
                kind: Kind::Bugzilla,
                id: "1234".to_string()
            }
        );
        assert_eq!(
            parse_ref(&e, "jira:RHEL-99").unwrap(),
            IssueRef {
                kind: Kind::Jira,
                id: "RHEL-99".to_string()
            }
        );
        // A bare number follows the configured issue tracker.
        assert_eq!(
            parse_ref(&e, "42").unwrap(),
            IssueRef {
                kind: Kind::Pagure,
                id: "42".to_string()
            }
        );
        assert!(parse_ref(&e, "nope:1").is_err());
        assert!(parse_ref(&e, "  ").is_err());
    }

    #[test]
    fn find_refs_extracts_and_dedups_from_free_text() {
        let e = env();
        let text = "Resolves https://bugzilla.example/show_bug.cgi?id=1234 and \
                    https://bugzilla.example/show_bug.cgi?id=1234 plus \
                    https://jira.example/browse/RHEL-99 (see also \
                    https://pagure.example/freeipa/issues/42).\n\
                    Unrelated https://other.example/x/7";
        let found = find_refs(&e, text)
            .iter()
            .map(|r| (r.kind, r.id.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            found,
            vec![
                (Kind::Bugzilla, "1234".to_string()),
                (Kind::Jira, "RHEL-99".to_string()),
                (Kind::Pagure, "42".to_string()),
            ]
        );
        assert!(find_refs(&e, "no links here").is_empty());
    }

    #[test]
    fn build_body_marks_origin_and_takes_last_comments() {
        let src = SourceContent {
            title: "t".to_string(),
            body: "  body text  ".to_string(),
            comments: vec!["c1".to_string(), "c2".to_string(), "c3".to_string()],
            origin_url: "https://bugzilla.example/show_bug.cgi?id=1".to_string(),
        };
        let body = build_body(&src, 2);
        assert!(body.starts_with("Cloned from: https://bugzilla.example/show_bug.cgi?id=1"));
        assert!(body.contains("body text"));
        assert!(!body.trim_end().ends_with("c1"), "{body}");
        // Last-two ordering is chronological.
        let i2 = body.find("c2").unwrap();
        let i3 = body.find("c3").unwrap();
        assert!(i2 < i3);
        // Asking for more comments than exist is not an error.
        assert!(build_body(&src, 9).contains("c1"));
        assert!(!build_body(&src, 0).contains("c1"));
    }

    #[test]
    fn target_selection_reflects_configuration() {
        let mut e = env();
        assert!(available_targets(&e).is_empty());
        assert!(preferred_target(&e).is_none());

        e.config.bugzilla_product = "Red Hat Enterprise Linux".to_string();
        e.bugzilla = Some(Arc::new(
            BugzillaClient::new("https://bugzilla.example", "tok").unwrap(),
        ));
        e.jira = Some(Arc::new(
            JiraClient::new("https://jira.example", "tok").unwrap(),
        ));
        // jira-ticket-url has no /browse/<PROJECT>- part, so Jira is unusable.
        let targets = available_targets(&e);
        assert!(targets.contains(&Kind::Bugzilla), "{targets:?}");
        assert!(!targets.contains(&Kind::Jira), "{targets:?}");
        assert_eq!(preferred_target(&e), Some(Kind::Bugzilla));
        // With a derivable project key Jira wins (upstream → downstream flow).
        e.config.jira_ticket_url = "https://jira.example/browse/RHEL-".to_string();
        assert_eq!(preferred_target(&e), Some(Kind::Jira));
    }

    #[test]
    fn same_tracker_source_and_target_is_rejected() {
        let mut e = env();
        e.config.bugzilla_product = "Red Hat Enterprise Linux".to_string();
        e.bugzilla = Some(Arc::new(
            BugzillaClient::new("https://bugzilla.example", "tok").unwrap(),
        ));
        let params = CloneParams {
            source: "bugzilla:1",
            to: Kind::Bugzilla,
            comments: 0,
            title: None,
            yes: true,
        };
        let err = prepare(&e, &params).err().unwrap().to_string();
        assert!(err.contains("both Bugzilla"), "{err}");
    }
}
