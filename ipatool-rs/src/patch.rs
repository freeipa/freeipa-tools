use anyhow::{bail, Result};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static SUBJECT_RE: OnceLock<Regex> = OnceLock::new();
static REVIEWER_LINE_RE: OnceLock<Regex> = OnceLock::new();

fn subject_re() -> &'static Regex {
    SUBJECT_RE
        .get_or_init(|| Regex::new(r"^Subject:(?:\s*\[PATCH[^\]]*\])*\s*(?P<subj>.*)").unwrap())
}

fn reviewer_line_re() -> &'static Regex {
    REVIEWER_LINE_RE.get_or_init(|| Regex::new(r"^[-_a-zA-Z0-9]+: .*$").unwrap())
}

/// A sanitized patch ready for application
pub struct Patch {
    pub filename: PathBuf,
    pub subject: String,
    pub ticket_numbers: Vec<u64>,
    head_lines: Vec<String>,
    patch_lines: Vec<String>,
}

impl Patch {
    pub fn from_file(path: &Path, ticket_url: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Cannot read patch {}: {}", path.display(), e))?;
        Self::from_content(path.to_path_buf(), &content, ticket_url)
    }

    pub fn from_content(filename: PathBuf, content: &str, ticket_url: &str) -> Result<Self> {
        if content.is_empty() {
            bail!("Empty patch: {}", filename.display());
        }

        let mut head_lines: Vec<String> = Vec::new();
        let mut patch_lines: Vec<String> = Vec::new();
        let mut subject = String::new();
        let mut in_subject = false;
        let mut found_patch_start = false;

        for line in content.lines() {
            let line_with_nl = format!("{}\n", line);

            if !line.starts_with(' ') {
                in_subject = false;
            }
            if in_subject {
                subject.push_str(line.trim_end());
            }

            if let Some(caps) = subject_re().captures(line) {
                subject = caps
                    .name("subj")
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default();
                in_subject = true;
            }

            if found_patch_start {
                patch_lines.push(line_with_nl);
                continue;
            }

            if line.starts_with(">From") {
                head_lines.push(format!("{}\n", &line[1..]));
            } else if line == "---" || line.starts_with("diff -") || line.starts_with("Index: ") {
                patch_lines.push(line_with_nl);
                found_patch_start = true;
            } else {
                head_lines.push(line_with_nl);
            }
        }

        // Extract ticket numbers from head lines (skip removed/context lines)
        let mut ticket_numbers = Vec::new();
        if !ticket_url.is_empty() {
            let escaped = regex::escape(ticket_url);
            if let Ok(ticket_re) = Regex::new(&format!(r"{}(\d+)", escaped)) {
                for line in &head_lines {
                    if line.starts_with('-') || line.starts_with(' ') {
                        continue;
                    }
                    for cap in ticket_re.captures_iter(line) {
                        if let Some(m) = cap.get(1) {
                            if let Ok(n) = m.as_str().parse::<u64>() {
                                if !ticket_numbers.contains(&n) {
                                    ticket_numbers.push(n);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(Patch {
            filename,
            subject,
            ticket_numbers,
            head_lines,
            patch_lines,
        })
    }

    pub fn add_reviewer(&mut self, reviewer: &str) {
        // If last head_line doesn't look like a header field, add blank line first
        if let Some(last) = self.head_lines.last() {
            if !reviewer_line_re().is_match(last.trim_end()) {
                self.head_lines.push("\n".to_string());
            }
        }
        self.head_lines.push(format!("Reviewed-By: {}\n", reviewer));
    }

    pub fn content(&self) -> String {
        let mut out = String::new();
        for line in &self.head_lines {
            out.push_str(line);
        }
        for line in &self.patch_lines {
            out.push_str(line);
        }
        out
    }

    pub fn display_name(&self) -> String {
        self.filename.display().to_string()
    }
}

/// Get all patches from a list of paths (files or directories)
pub fn collect_patches(paths: &[String], patchdir: &Path, ticket_url: &str) -> Result<Vec<Patch>> {
    let effective_paths: Vec<String> = if paths.is_empty() {
        vec![patchdir.to_string_lossy().to_string()]
    } else {
        paths.to_vec()
    };

    let mut patches = Vec::new();
    for path_str in &effective_paths {
        let path = crate::config::expand_path(path_str);
        if path.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&path)
                .map_err(|e| anyhow::anyhow!("Cannot read dir {}: {}", path.display(), e))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|ext| ext == "patch").unwrap_or(false))
                .collect();
            entries.sort();
            for entry in entries {
                patches.push(Patch::from_file(&entry, ticket_url)?);
            }
        } else {
            patches.push(Patch::from_file(&path, ticket_url)?);
        }
    }
    Ok(patches)
}

/// Generate a patch filename from a commit message and sequence number
pub fn patch_filename(msg: &str, num: usize) -> String {
    let title = msg.split("\n\n").next().unwrap_or("");
    let title = title.replace(' ', "-");
    let title: String = title
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    format!("{:04}-{}.patch", num, title)
}

/// Delete all .patch files in a directory
pub fn delete_patches(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "patch").unwrap_or(false) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_str(content: &str) -> Result<Patch> {
        Patch::from_content(
            PathBuf::from("test.patch"),
            content,
            "https://pagure.io/freeipa/issue/",
        )
    }

    // Standard minimal patch (subject without PATCH tag)
    const SIMPLE_PATCH: &str = "\
From abc123 Mon Sep 17 00:00:00 2001
From: Developer <dev@example.com>
Date: Mon, 1 Jan 2024 10:00:00 +0000
Subject: Fix something important

More description here.

Signed-off-by: Developer <dev@example.com>
---
 src/main.rs | 2 +-
 1 file changed

diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1 +1 @@
-old line
+new line
";

    // ── patch_filename ────────────────────────────────────────────────────────

    #[test]
    fn test_patch_filename_basic() {
        assert_eq!(
            patch_filename("Fix LDAP timeout", 1),
            "0001-Fix-LDAP-timeout.patch"
        );
    }

    #[test]
    fn test_patch_filename_only_first_line() {
        // Only text before the first "\n\n" is used
        let name = patch_filename("Fix issue\n\nLonger description here", 42);
        assert_eq!(name, "0042-Fix-issue.patch");
    }

    #[test]
    fn test_patch_filename_special_chars_stripped() {
        let name = patch_filename("Fix (some) issue! With: punctuation", 5);
        // Parentheses, exclamation mark, colon are stripped; spaces → dashes
        assert_eq!(name, "0005-Fix-some-issue-With-punctuation.patch");
    }

    #[test]
    fn test_patch_filename_leading_zeros() {
        assert!(patch_filename("Fix", 3).starts_with("0003-"));
    }

    #[test]
    fn test_patch_filename_four_digit_seq() {
        let name = patch_filename("Big change", 1234);
        assert!(name.starts_with("1234-"));
    }

    #[test]
    fn test_patch_filename_empty_message() {
        // Empty message → only sequence number and extension
        let name = patch_filename("", 1);
        assert_eq!(name, "0001-.patch");
    }

    // ── Patch::from_content – subject extraction ──────────────────────────────

    #[test]
    fn test_from_content_subject_plain() {
        let patch = from_str(SIMPLE_PATCH).unwrap();
        assert_eq!(patch.subject, "Fix something important");
    }

    #[test]
    fn test_from_content_subject_with_patch_tag() {
        let content = "\
From abc Mon Sep 17 00:00:00 2001
Subject: [PATCH] Refactor the module
---
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1 +1 @@
-a
+b
";
        let patch = Patch::from_content(PathBuf::from("0001.patch"), content, "").unwrap();
        assert_eq!(patch.subject, "Refactor the module");
    }

    #[test]
    fn test_from_content_subject_with_numbered_patch_tag() {
        let content = "\
From abc Mon Sep 17 00:00:00 2001
Subject: [PATCH 2/3] Add new feature
---
diff --git a/y b/y
--- a/y
+++ b/y
@@ -1 +1 @@
-a
+b
";
        let patch = Patch::from_content(PathBuf::from("0002.patch"), content, "").unwrap();
        assert_eq!(patch.subject, "Add new feature");
    }

    #[test]
    fn test_from_content_empty_fails() {
        let result = from_str("");
        assert!(result.is_err());
    }

    // ── Patch::from_content – >From escaping ──────────────────────────────────

    #[test]
    fn test_from_content_escaped_from() {
        // Some mail clients escape "From" at the start of a line as ">From"
        let content = "\
>From abc123 Mon Sep 17 00:00:00 2001
From: Dev <dev@example.com>
Subject: Test patch
---
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1 +1 @@
-a
+b
";
        let patch = from_str(content).unwrap();
        let c = patch.content();
        // The ">From" should become "From" in the output header
        assert!(c.contains("From abc123"));
        assert!(!c.contains(">From abc123"));
    }

    // ── Patch::add_reviewer ───────────────────────────────────────────────────

    #[test]
    fn test_add_reviewer() {
        let mut patch = from_str(SIMPLE_PATCH).unwrap();
        patch.add_reviewer("Alice Smith <alice@example.com>");
        let content = patch.content();
        assert!(content.contains("Reviewed-By: Alice Smith <alice@example.com>"));
    }

    #[test]
    fn test_add_multiple_reviewers() {
        let mut patch = from_str(SIMPLE_PATCH).unwrap();
        patch.add_reviewer("Alice <alice@example.com>");
        patch.add_reviewer("Bob <bob@example.com>");
        let content = patch.content();
        assert!(content.contains("Reviewed-By: Alice <alice@example.com>"));
        assert!(content.contains("Reviewed-By: Bob <bob@example.com>"));
    }

    // ── Patch::content – structure ────────────────────────────────────────────

    #[test]
    fn test_content_contains_diff() {
        let patch = from_str(SIMPLE_PATCH).unwrap();
        let content = patch.content();
        assert!(content.contains("diff --git"));
        assert!(content.contains("+new line"));
        assert!(content.contains("-old line"));
    }

    #[test]
    fn test_content_preserves_header() {
        let patch = from_str(SIMPLE_PATCH).unwrap();
        let content = patch.content();
        assert!(content.contains("Developer <dev@example.com>"));
        assert!(content.contains("Fix something important"));
    }

    // ── Ticket number extraction ──────────────────────────────────────────────

    #[test]
    fn test_ticket_extraction() {
        let content = "\
From abc Mon Sep 17 00:00:00 2001
From: Dev <dev@example.com>
Subject: Fix issue

Fixes: https://pagure.io/freeipa/issue/9000
Also: https://pagure.io/freeipa/issue/8999
---
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1 +1 @@
-a
+b
";
        let patch = Patch::from_content(
            PathBuf::from("test.patch"),
            content,
            "https://pagure.io/freeipa/issue/",
        )
        .unwrap();
        assert!(patch.ticket_numbers.contains(&9000));
        assert!(patch.ticket_numbers.contains(&8999));
    }

    #[test]
    fn test_ticket_extraction_deduplicates() {
        let content = "\
From abc Mon Sep 17 00:00:00 2001
Subject: Fix issue

Refs: https://pagure.io/freeipa/issue/1234
Also: https://pagure.io/freeipa/issue/1234
---
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1 +1 @@
-a
+b
";
        let patch = Patch::from_content(
            PathBuf::from("test.patch"),
            content,
            "https://pagure.io/freeipa/issue/",
        )
        .unwrap();
        assert_eq!(
            patch.ticket_numbers.iter().filter(|&&n| n == 1234).count(),
            1
        );
    }

    #[test]
    fn test_no_ticket_url_gives_empty() {
        let patch = Patch::from_content(PathBuf::from("test.patch"), SIMPLE_PATCH, "").unwrap();
        assert!(patch.ticket_numbers.is_empty());
    }

    #[test]
    fn test_ticket_not_extracted_from_diff_lines() {
        // Lines starting with '-' or ' ' (context) should be skipped
        let content = "\
From abc Mon Sep 17 00:00:00 2001
Subject: Fix issue
---
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1,2 +1,2 @@
-https://pagure.io/freeipa/issue/9999
+https://pagure.io/freeipa/issue/9999
";
        let patch = Patch::from_content(
            PathBuf::from("test.patch"),
            content,
            "https://pagure.io/freeipa/issue/",
        )
        .unwrap();
        // Diff lines are in patch_lines, not head_lines, so no extraction
        assert!(patch.ticket_numbers.is_empty());
    }

    // ── delete_patches ────────────────────────────────────────────────────────

    #[test]
    fn test_delete_patches() {
        let dir = tempfile::tempdir().unwrap();
        let patch_path = dir.path().join("0001-test.patch");
        let other_path = dir.path().join("notes.txt");
        std::fs::write(&patch_path, "patch content").unwrap();
        std::fs::write(&other_path, "notes").unwrap();

        delete_patches(dir.path());

        assert!(!patch_path.exists(), ".patch file should be deleted");
        assert!(other_path.exists(), "non-patch file should remain");
    }

    #[test]
    fn test_delete_patches_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        // Should not panic on empty directory
        delete_patches(dir.path());
    }
}
