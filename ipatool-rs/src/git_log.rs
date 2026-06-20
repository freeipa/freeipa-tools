use anyhow::Result;
use regex::Regex;
use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

static COMMIT_RE: OnceLock<Regex> = OnceLock::new();
static AUTHOR_RE: OnceLock<Regex> = OnceLock::new();
static DATE_RE: OnceLock<Regex> = OnceLock::new();
static DESCRIPTION_RE: OnceLock<Regex> = OnceLock::new();
static REVIEWER_RE: OnceLock<Regex> = OnceLock::new();
static RELEASENOTE_RE: OnceLock<Regex> = OnceLock::new();

fn commit_re() -> &'static Regex {
    COMMIT_RE.get_or_init(|| Regex::new(r"^commit (\w+)$").expect("COMMIT_RE pattern is valid"))
}
fn author_re() -> &'static Regex {
    AUTHOR_RE
        .get_or_init(|| Regex::new(r"^Author: (.+) <(.*)>$").expect("AUTHOR_RE pattern is valid"))
}
fn date_re() -> &'static Regex {
    DATE_RE.get_or_init(|| Regex::new(r"^Date:\s+(.+)$").expect("DATE_RE pattern is valid"))
}
fn description_re() -> &'static Regex {
    DESCRIPTION_RE
        .get_or_init(|| Regex::new(r"^    (.*)$").expect("DESCRIPTION_RE pattern is valid"))
}
fn reviewer_re() -> &'static Regex {
    REVIEWER_RE.get_or_init(|| {
        Regex::new(r"^\s*Reviewed-By: (.+) <(.+)>$").expect("REVIEWER_RE pattern is valid")
    })
}
fn releasenote_re() -> &'static Regex {
    RELEASENOTE_RE
        .get_or_init(|| Regex::new(r"^\s*RN:\s+(.*)$").expect("RELEASENOTE_RE pattern is valid"))
}

#[derive(Debug, Clone)]
pub(crate) struct GitCommit {
    pub(crate) hash: String,
    pub(crate) author_name: String,
    pub(crate) author_email: String,
    pub(crate) date: String,
    pub(crate) summary: String,
    pub(crate) description: String,
    pub(crate) tickets: HashSet<u64>,
    pub(crate) release_notes: Vec<String>,
    pub(crate) reviewers: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub(crate) struct GitAuthor {
    pub(crate) name: String,
    pub(crate) email: String,
    pub(crate) commit_indices: Vec<usize>,
    pub(crate) review_indices: Vec<usize>,
    pub(crate) tickets: HashSet<u64>,
}

#[derive(Debug)]
pub(crate) struct GitLogResult {
    pub(crate) commits: Vec<GitCommit>,
    pub(crate) authors: BTreeMap<String, GitAuthor>,
}

fn build_ticket_regexes(ticket_url: &str, legacy_ticket_url: &str) -> Vec<Regex> {
    let mut regexes = Vec::new();
    for url in [ticket_url, legacy_ticket_url] {
        if url.is_empty() {
            continue;
        }
        let escaped = regex::escape(url);
        match Regex::new(&format!(r"{}(\d+)", escaped)) {
            Ok(re) => regexes.push(re),
            Err(e) => {
                eprintln!(
                    "Warning: failed to compile ticket regex for URL '{}': {}",
                    url, e
                );
            }
        }
    }
    regexes
}

pub(crate) fn parse_git_log(
    output: &str,
    ticket_url: &str,
    legacy_ticket_url: &str,
) -> GitLogResult {
    let ticket_regexes = build_ticket_regexes(ticket_url, legacy_ticket_url);

    let mut commits: Vec<GitCommit> = Vec::new();
    let mut description_lines: Vec<String> = Vec::new();
    let mut current: Option<GitCommit> = None;

    for line in output.lines() {
        if let Some(caps) = commit_re().captures(line) {
            if let Some(mut prev) = current.take() {
                prev.description = description_lines.join("\n");
                if !description_lines.is_empty() {
                    prev.summary = description_lines[0].clone();
                }
                parse_description(&mut prev, &ticket_regexes);
                commits.push(prev);
            }
            description_lines.clear();
            current = Some(GitCommit {
                hash: caps[1].to_string(),
                author_name: String::new(),
                author_email: String::new(),
                date: String::new(),
                summary: String::new(),
                description: String::new(),
                tickets: HashSet::new(),
                release_notes: Vec::new(),
                reviewers: Vec::new(),
            });
            continue;
        }

        let commit = match current.as_mut() {
            Some(c) => c,
            None => continue,
        };

        if let Some(caps) = author_re().captures(line) {
            commit.author_name = caps[1].to_string();
            commit.author_email = caps[2].to_string();
        } else if let Some(caps) = date_re().captures(line) {
            commit.date = caps[1].trim().to_string();
        } else if let Some(caps) = description_re().captures(line) {
            description_lines.push(caps[1].to_string());
        }
    }

    if let Some(mut last) = current.take() {
        last.description = description_lines.join("\n");
        if !description_lines.is_empty() {
            last.summary = description_lines[0].clone();
        }
        parse_description(&mut last, &ticket_regexes);
        commits.push(last);
    }

    let authors = build_author_map(&commits);

    GitLogResult { commits, authors }
}

fn parse_description(commit: &mut GitCommit, ticket_regexes: &[Regex]) {
    for line in commit.description.lines() {
        if let Some(caps) = reviewer_re().captures(line) {
            commit
                .reviewers
                .push((caps[1].to_string(), caps[2].to_string()));
        }
        if let Some(caps) = releasenote_re().captures(line) {
            commit.release_notes.push(caps[1].to_string());
        }
        for re in ticket_regexes {
            for caps in re.captures_iter(line) {
                if let Some(m) = caps.get(1) {
                    if let Ok(n) = m.as_str().parse::<u64>() {
                        commit.tickets.insert(n);
                    }
                }
            }
        }
    }
}

fn build_author_map(commits: &[GitCommit]) -> BTreeMap<String, GitAuthor> {
    let mut authors: BTreeMap<String, GitAuthor> = BTreeMap::new();

    for (idx, commit) in commits.iter().enumerate() {
        let email = if commit.author_email.is_empty() {
            find_email_by_name(&authors, &commit.author_name)
                .unwrap_or_else(|| "unidentified".to_string())
        } else {
            commit.author_email.clone()
        };

        let author = authors.entry(email.clone()).or_insert_with(|| GitAuthor {
            name: commit.author_name.clone(),
            email,
            commit_indices: Vec::new(),
            review_indices: Vec::new(),
            tickets: HashSet::new(),
        });
        author.commit_indices.push(idx);
        author.tickets.extend(&commit.tickets);
    }

    for (idx, commit) in commits.iter().enumerate() {
        for (name, email) in &commit.reviewers {
            let reviewer_email = if email.is_empty() {
                find_email_by_name(&authors, name).unwrap_or_else(|| "unidentified".to_string())
            } else {
                email.clone()
            };
            let author = authors
                .entry(reviewer_email.clone())
                .or_insert_with(|| GitAuthor {
                    name: name.clone(),
                    email: reviewer_email,
                    commit_indices: Vec::new(),
                    review_indices: Vec::new(),
                    tickets: HashSet::new(),
                });
            author.review_indices.push(idx);
        }
    }

    authors
}

fn find_email_by_name(authors: &BTreeMap<String, GitAuthor>, name: &str) -> Option<String> {
    authors
        .values()
        .find(|a| a.name == name)
        .map(|a| a.email.clone())
}

pub(crate) fn run_git_log(
    range: &str,
    repo_path: &str,
    env: &std::collections::HashMap<String, String>,
    verbosity: u8,
) -> Result<String> {
    let result = crate::git::run_process(
        &["git", "-C", repo_path, "log", "--use-mailmap", range],
        env,
        None,
        true,
        None,
        verbosity,
    )?;
    Ok(result.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_LOG: &str = "\
commit abc123def456
Author: Alice Developer <alice@example.com>
Date:   Mon Jan 1 10:00:00 2024 +0000

    Fix LDAP timeout handling

    When the LDAP server is unreachable, the client now properly
    times out after the configured interval.

    Reviewed-By: Bob Reviewer <bob@example.com>
    RN: Fixed LDAP timeout handling for unreachable servers
    Fixes: https://pagure.io/freeipa/issue/9000

commit def789abc012
Author: Carol Coder <carol@example.com>
Date:   Tue Jan 2 11:00:00 2024 +0000

    Add new KDC configuration option

    This adds a new option to configure the KDC renewal lifetime.

    RN: Added KDC renewal lifetime configuration
    RN: Administrators can now set kdc_renewal_lifetime in krb5.conf
    Reviewed-By: Alice Developer <alice@example.com>
    Fixes: https://pagure.io/freeipa/issue/9001
    Fixes: https://pagure.io/freeipa/issue/9002
";

    #[test]
    fn test_parse_commits_count() {
        let result = parse_git_log(SAMPLE_LOG, "https://pagure.io/freeipa/issue/", "");
        assert_eq!(result.commits.len(), 2);
    }

    #[test]
    fn test_parse_commit_hash() {
        let result = parse_git_log(SAMPLE_LOG, "https://pagure.io/freeipa/issue/", "");
        assert_eq!(result.commits[0].hash, "abc123def456");
        assert_eq!(result.commits[1].hash, "def789abc012");
    }

    #[test]
    fn test_parse_commit_author() {
        let result = parse_git_log(SAMPLE_LOG, "https://pagure.io/freeipa/issue/", "");
        assert_eq!(result.commits[0].author_name, "Alice Developer");
        assert_eq!(result.commits[0].author_email, "alice@example.com");
    }

    #[test]
    fn test_parse_commit_date() {
        let result = parse_git_log(SAMPLE_LOG, "https://pagure.io/freeipa/issue/", "");
        assert_eq!(result.commits[0].date, "Mon Jan 1 10:00:00 2024 +0000");
    }

    #[test]
    fn test_parse_commit_summary() {
        let result = parse_git_log(SAMPLE_LOG, "https://pagure.io/freeipa/issue/", "");
        assert_eq!(result.commits[0].summary, "Fix LDAP timeout handling");
        assert_eq!(
            result.commits[1].summary,
            "Add new KDC configuration option"
        );
    }

    #[test]
    fn test_parse_ticket_references() {
        let result = parse_git_log(SAMPLE_LOG, "https://pagure.io/freeipa/issue/", "");
        assert!(result.commits[0].tickets.contains(&9000));
        assert_eq!(result.commits[0].tickets.len(), 1);

        assert!(result.commits[1].tickets.contains(&9001));
        assert!(result.commits[1].tickets.contains(&9002));
        assert_eq!(result.commits[1].tickets.len(), 2);
    }

    #[test]
    fn test_parse_release_notes() {
        let result = parse_git_log(SAMPLE_LOG, "https://pagure.io/freeipa/issue/", "");
        assert_eq!(result.commits[0].release_notes.len(), 1);
        assert_eq!(
            result.commits[0].release_notes[0],
            "Fixed LDAP timeout handling for unreachable servers"
        );

        assert_eq!(result.commits[1].release_notes.len(), 2);
        assert_eq!(
            result.commits[1].release_notes[0],
            "Added KDC renewal lifetime configuration"
        );
    }

    #[test]
    fn test_parse_reviewers() {
        let result = parse_git_log(SAMPLE_LOG, "https://pagure.io/freeipa/issue/", "");
        assert_eq!(result.commits[0].reviewers.len(), 1);
        assert_eq!(result.commits[0].reviewers[0].0, "Bob Reviewer");
        assert_eq!(result.commits[0].reviewers[0].1, "bob@example.com");

        assert_eq!(result.commits[1].reviewers.len(), 1);
        assert_eq!(result.commits[1].reviewers[0].0, "Alice Developer");
    }

    #[test]
    fn test_author_map() {
        let result = parse_git_log(SAMPLE_LOG, "https://pagure.io/freeipa/issue/", "");
        assert!(result.authors.contains_key("alice@example.com"));
        assert!(result.authors.contains_key("carol@example.com"));
        assert!(result.authors.contains_key("bob@example.com"));

        let alice = &result.authors["alice@example.com"];
        assert_eq!(alice.commit_indices.len(), 1);
        assert_eq!(alice.review_indices.len(), 1);

        let bob = &result.authors["bob@example.com"];
        assert_eq!(bob.commit_indices.len(), 0);
        assert_eq!(bob.review_indices.len(), 1);
    }

    #[test]
    fn test_author_tickets() {
        let result = parse_git_log(SAMPLE_LOG, "https://pagure.io/freeipa/issue/", "");
        let alice = &result.authors["alice@example.com"];
        assert!(alice.tickets.contains(&9000));

        let carol = &result.authors["carol@example.com"];
        assert!(carol.tickets.contains(&9001));
        assert!(carol.tickets.contains(&9002));
    }

    #[test]
    fn test_legacy_ticket_url() {
        let log = "\
commit aaa111
Author: Dev <dev@example.com>
Date:   Mon Jan 1 10:00:00 2024 +0000

    Fix old ticket

    Fixes: https://fedorahosted.org/freeipa/ticket/5000
    Fixes: https://pagure.io/freeipa/issue/9000
";
        let result = parse_git_log(
            log,
            "https://pagure.io/freeipa/issue/",
            "https://fedorahosted.org/freeipa/ticket/",
        );
        assert!(result.commits[0].tickets.contains(&5000));
        assert!(result.commits[0].tickets.contains(&9000));
    }

    #[test]
    fn test_empty_log() {
        let result = parse_git_log("", "https://pagure.io/freeipa/issue/", "");
        assert!(result.commits.is_empty());
        assert!(result.authors.is_empty());
    }

    #[test]
    fn test_commit_without_ticket() {
        let log = "\
commit fff999
Author: Dev <dev@example.com>
Date:   Mon Jan 1 10:00:00 2024 +0000

    Improve documentation

    Update README with better examples.
";
        let result = parse_git_log(log, "https://pagure.io/freeipa/issue/", "");
        assert_eq!(result.commits.len(), 1);
        assert!(result.commits[0].tickets.is_empty());
        assert!(result.commits[0].release_notes.is_empty());
        assert!(result.commits[0].reviewers.is_empty());
    }

    #[test]
    fn test_no_ticket_url_configured() {
        let result = parse_git_log(SAMPLE_LOG, "", "");
        assert_eq!(result.commits.len(), 2);
        assert!(result.commits[0].tickets.is_empty());
        // Release notes and reviewers still extracted
        assert_eq!(result.commits[0].release_notes.len(), 1);
        assert_eq!(result.commits[0].reviewers.len(), 1);
    }
}
