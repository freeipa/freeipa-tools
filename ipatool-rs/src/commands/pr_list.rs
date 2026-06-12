use anyhow::Result;
use std::collections::HashSet;

use super::Ctx;
use crate::api::types::labels_colorize;

pub fn run(ctx: &Ctx, state_args: &[String], label_args: &[String]) -> Result<()> {
    let prc = ctx.pr_client_or_err()?;
    let color_enabled = ctx.out.color.enabled();

    // Parse +/- prefixed state and label filters
    let (states_pos, states_neg) = parse_signed(state_args);
    let (labels_pos, labels_neg) = parse_signed(label_args);

    // Contradictory filters → no results
    if !states_pos.is_disjoint(&states_neg) || !labels_pos.is_disjoint(&labels_neg) {
        return Ok(());
    }
    if states_neg.contains("all") {
        return Ok(());
    }

    let search_state = if states_neg.is_empty() {
        if states_pos.is_empty() {
            "open".to_string()
        } else if states_pos.len() == 1 {
            states_pos.iter().next().unwrap().clone()
        } else {
            "all".to_string()
        }
    } else {
        "all".to_string()
    };

    // Discard "all" as a real state filter
    let states_pos: HashSet<String> = states_pos.into_iter().filter(|s| s != "all").collect();

    let prs = prc.list_prs(&search_state)?;

    // Keep the local cache current as a side-effect of the online fetch so
    // that `cache-update` is not required for basic offline availability.
    if let Some(ref db) = ctx.db {
        if let Err(e) = db.cache_prs(&ctx.profile, &prs) {
            eprintln!("Warning: failed to update PR cache: {}", e);
        }
    }

    for pr in &prs {
        // State filtering
        if !states_pos.is_empty() && !states_pos.contains(&pr.state) {
            continue;
        }
        if states_neg.contains(&pr.state) {
            continue;
        }

        let labels = prc.get_pr_labels(pr.number)?;
        let label_names: HashSet<String> = labels.iter().map(|l| l.name.clone()).collect();

        // Label filtering
        if !labels_pos.is_empty() && labels_pos.is_disjoint(&label_names) {
            continue;
        }
        if !labels_neg.is_disjoint(&label_names) {
            continue;
        }

        // Get CI statuses
        let statuses = prc.most_recent_statuses(&pr.head.sha)?;
        let statuses_display = if statuses.is_empty() {
            String::new()
        } else {
            let mut parts: Vec<String> = statuses
                .iter()
                .map(|(k, v)| format!("{}:{}", k, v.state))
                .collect();
            parts.sort();
            format!("{{{}}}", parts.join(", "))
        };

        let labels_str = labels_colorize(&labels, color_enabled);
        println!(
            "{:5}\t{:.50}\t{}\t{}\t{}",
            pr.number, pr.title, labels_str, pr.html_url, statuses_display
        );
    }

    // Human error detection: last 100 updated PRs
    ctx.out.print_colored(
        "Checking for common mistakes...",
        crate::output::Color::Rgb(0xff, 0x33, 0x11),
    );

    // Phase 1: fetch PR list, showing page progress so the user sees activity.
    let checked = prc.list_prs_limited("all", 100, |page, collected| {
        fetch_progress(page, collected, color_enabled);
    })?;
    fetch_progress_clear();

    // Phase 2: per-PR checks with a progress bar.
    let total = checked.len();
    let mut bar = ProgressBar::new(total, color_enabled);
    for (i, pr) in checked.iter().enumerate() {
        bar.render(i + 1, pr.number);

        let labels = prc.get_pr_labels(pr.number)?;
        let label_names: HashSet<String> = labels.iter().map(|l| l.name.clone()).collect();
        let is_merged = prc.is_pr_merged(pr.number).unwrap_or(false);

        bar.clear();

        if is_merged && !label_names.contains("pushed") {
            ctx.out
                .print_red("Pull request was merged but not labeled 'pushed'!");
            let labels_str = labels_colorize(&labels, color_enabled);
            println!(
                "{:5}\t{:.50}\t{}\t{}",
                pr.number, pr.title, labels_str, pr.html_url
            );
        }
        if label_names.contains("pushed") && !label_names.contains("ack") {
            ctx.out.print_red("Pull request was pushed without 'ack'!");
            let labels_str = labels_colorize(&labels, color_enabled);
            println!(
                "{:5}\t{:.50}\t{}\t{}",
                pr.number, pr.title, labels_str, pr.html_url
            );
        }
    }
    bar.finish(total);

    Ok(())
}

fn fetch_progress(page: u32, collected: usize, color: bool) {
    use std::io::Write;
    let line = if color {
        format!(
            "\r\x1b[36mFetching page {} ({} PRs so far)...\x1b[0m\x1b[K",
            page, collected
        )
    } else {
        format!(
            "\rFetching page {} ({} PRs so far)...\x1b[K",
            page, collected
        )
    };
    eprint!("{}", line);
    let _ = std::io::stderr().flush();
}

fn fetch_progress_clear() {
    use std::io::Write;
    eprint!("\r\x1b[K");
    let _ = std::io::stderr().flush();
}

/// Simple terminal progress bar drawn to stderr.
/// Uses `\r` to overwrite the current line.
struct ProgressBar {
    total: usize,
    color: bool,
    /// Width of the bar fill area (between brackets)
    width: usize,
    last_len: usize,
}

impl ProgressBar {
    fn new(total: usize, color: bool) -> Self {
        ProgressBar {
            total,
            color,
            width: 30,
            last_len: 0,
        }
    }

    /// Render the bar for item `done` (0-based) which is PR `pr_num`.
    fn render(&mut self, done: usize, pr_num: u64) {
        use std::io::Write;
        let filled = (done * self.width).checked_div(self.total).unwrap_or(0);
        let pct = (done * 100).checked_div(self.total).unwrap_or(0);

        let bar = format!(
            "[{}{}] {}/{} PR #{}",
            "=".repeat(filled),
            " ".repeat(self.width - filled),
            done,
            self.total,
            pr_num,
        );

        let line = if self.color {
            format!("\r\x1b[36m{}\x1b[0m  {:3}%", bar, pct)
        } else {
            format!("\r{}  {:3}%", bar, pct)
        };

        self.last_len = line.len();
        eprint!("{}", line);
        let _ = std::io::stderr().flush();
    }

    /// Erase the progress line so output can print above it.
    fn clear(&self) {
        use std::io::Write;
        eprint!("\r{}\r", " ".repeat(self.last_len));
        let _ = std::io::stderr().flush();
    }

    /// Replace bar with a completion message.
    fn finish(&self, done: usize) {
        use std::io::Write;
        let line = if self.color {
            format!(
                "\r\x1b[32m[{}] {}/{} done\x1b[0m\n",
                "=".repeat(self.width),
                done,
                self.total,
            )
        } else {
            format!(
                "\r[{}] {}/{} done\n",
                "=".repeat(self.width),
                done,
                self.total,
            )
        };
        eprint!("{}", line);
        let _ = std::io::stderr().flush();
    }
}

fn parse_signed(items: &[String]) -> (HashSet<String>, HashSet<String>) {
    let mut pos = HashSet::new();
    let mut neg = HashSet::new();
    for item in items {
        if let Some(s) = item.strip_prefix('-') {
            neg.insert(s.to_string());
        } else if let Some(s) = item.strip_prefix('+') {
            pos.insert(s.to_string());
        } else {
            pos.insert(item.clone());
        }
    }
    (pos, neg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sv(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn test_parse_signed_empty() {
        let (pos, neg) = parse_signed(&[]);
        assert!(pos.is_empty());
        assert!(neg.is_empty());
    }

    #[test]
    fn test_parse_signed_bare_positive() {
        let (pos, neg) = parse_signed(&[sv("open")]);
        assert!(pos.contains("open"));
        assert!(neg.is_empty());
    }

    #[test]
    fn test_parse_signed_plus_prefix() {
        let (pos, neg) = parse_signed(&[sv("+open")]);
        assert!(pos.contains("open"));
        assert!(neg.is_empty());
    }

    #[test]
    fn test_parse_signed_minus_prefix() {
        let (pos, neg) = parse_signed(&[sv("-closed")]);
        assert!(pos.is_empty());
        assert!(neg.contains("closed"));
    }

    #[test]
    fn test_parse_signed_mixed() {
        let items = vec![sv("open"), sv("-closed"), sv("+ack")];
        let (pos, neg) = parse_signed(&items);
        assert_eq!(pos.len(), 2);
        assert!(pos.contains("open"));
        assert!(pos.contains("ack"));
        assert_eq!(neg.len(), 1);
        assert!(neg.contains("closed"));
    }

    #[test]
    fn test_parse_signed_deduplicates() {
        let items = vec![sv("open"), sv("open"), sv("+open")];
        let (pos, neg) = parse_signed(&items);
        // HashSet deduplicates
        assert_eq!(pos.len(), 1);
        assert!(neg.is_empty());
    }

    #[test]
    fn test_parse_signed_all_negative() {
        let items = vec![sv("-open"), sv("-closed"), sv("-all")];
        let (pos, neg) = parse_signed(&items);
        assert!(pos.is_empty());
        assert_eq!(neg.len(), 3);
        assert!(neg.contains("open"));
        assert!(neg.contains("closed"));
        assert!(neg.contains("all"));
    }
}
