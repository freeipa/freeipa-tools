/// CI job viewer for FreeIPA PRCI (S3-website-hosted results).
///
/// Directory listings are custom Bootstrap HTML with fontawesome icons.
/// The pytest report is `report.html` with test data embedded as a JSON blob.
/// Log files are gzip-compressed plain text.
use anyhow::{Context, Result};
use cursive::{
    reexports::enumset::EnumSet,
    theme::{BaseColor, Color, ColorStyle, Effect, Style},
    utils::markup::StyledString,
};
use flate2::read::GzDecoder;
use regex::Regex;
use serde::Deserialize;
use std::{collections::HashMap, io::Read, time::Duration};

use super::{ArtifactEntry, CiJobViewer};

// ── Viewer struct ─────────────────────────────────────────────────────────────

pub struct PrciViewer {
    http: reqwest::blocking::Client,
}

impl PrciViewer {
    pub fn new() -> Self {
        let http = reqwest::blocking::Client::builder()
            .user_agent("ipatool/1.0")
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client");
        PrciViewer { http }
    }

    fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self
            .http
            .get(url)
            .send()
            .with_context(|| format!("GET {url}"))?;
        Ok(resp.bytes().context("reading response body")?.to_vec())
    }

    fn fetch_text(&self, url: &str) -> Result<String> {
        let bytes = self.fetch_bytes(url)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

impl CiJobViewer for PrciViewer {
    fn can_handle(&self, url: &str) -> bool {
        // FreeIPA PRCI stores results on an S3 static website in eu-central-1.
        url.contains("s3-website") && url.contains("amazonaws.com")
    }

    fn list_artifacts(&self, url: &str) -> Result<Vec<ArtifactEntry>> {
        let html = self.fetch_text(url)?;
        Ok(parse_directory_listing(&html))
    }

    fn fetch_content(&self, entry: &ArtifactEntry) -> Result<StyledString> {
        if entry.name.ends_with(".gz") {
            let bytes = self.fetch_bytes(&entry.url)?;
            let mut decoder = GzDecoder::new(&bytes[..]);
            let mut text = String::new();
            decoder
                .read_to_string(&mut text)
                .context("decompressing gzip content")?;
            return Ok(StyledString::plain(text));
        }

        let text = self.fetch_text(&entry.url)?;

        if entry.name == "report.html" || entry.name.ends_with(".html") {
            if let Some(s) = try_parse_pytest_report(&text) {
                return Ok(s);
            }
        }

        Ok(StyledString::plain(text))
    }

    fn default_entry<'a>(&self, entries: &'a [ArtifactEntry]) -> Option<&'a ArtifactEntry> {
        entries.iter().find(|e| e.name == "report.html")
    }
}

// ── Directory listing parser ──────────────────────────────────────────────────

/// Parse the PRCI Bootstrap/fontawesome directory listing HTML.
///
/// The page uses `fas fa-folder` for directories and `fas fa-file` for files.
/// All `href` values are absolute URLs.
fn parse_directory_listing(html: &str) -> Vec<ArtifactEntry> {
    // Match both folder and file entries; capture (kind, url, name).
    // The pattern `fa-caret-up` (parent directory) is intentionally excluded.
    let re = Regex::new(r#"fas fa-(folder|file)[^>]*><a href="([^"]+)">\s*([^<]+?)\s*</a>"#)
        .expect("static regex");

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for cap in re.captures_iter(html) {
        let kind = &cap[1];
        let url = cap[2].trim().to_string();
        let name = cap[3].trim().to_string();
        if name.is_empty() || url.is_empty() {
            continue;
        }
        let entry = ArtifactEntry {
            name,
            url,
            is_dir: kind == "folder",
        };
        if entry.is_dir {
            dirs.push(entry);
        } else {
            files.push(entry);
        }
    }

    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    files.sort_by(|a, b| a.name.cmp(&b.name));
    dirs.extend(files);
    dirs
}

// ── Pytest HTML report parser ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TestEntry {
    #[serde(rename = "testId")]
    test_id: String,
    result: String,
    duration: String,
}

#[derive(Debug, Deserialize)]
struct ReportData {
    #[serde(default)]
    title: String,
    #[serde(default)]
    environment: HashMap<String, serde_json::Value>,
    /// Map from test ID to a list of run records (usually one per test).
    tests: HashMap<String, Vec<TestEntry>>,
}

/// Attempt to parse a pytest HTML report and render it as a `StyledString`.
/// Returns `None` if the expected JSON blob is not found (not a pytest report).
fn try_parse_pytest_report(html: &str) -> Option<StyledString> {
    // The JSON blob lives in:  <div id="data-container" data-jsonblob="...">
    let re =
        Regex::new(r#"id="data-container"[^>]*data-jsonblob="([^"]+)""#).expect("static regex");
    let cap = re.captures(html)?;
    let raw = html_unescape(&cap[1]);
    let data: ReportData = serde_json::from_str(&raw).ok()?;
    Some(render_pytest_report(&data))
}

/// Minimal HTML entity unescaping sufficient for the JSON blob.
fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

// ── Style helpers ─────────────────────────────────────────────────────────────

fn red() -> Style {
    Style::from(Color::Light(BaseColor::Red))
}

fn green() -> Style {
    Style::from(Color::Light(BaseColor::Green))
}

fn yellow() -> Style {
    Style::from(Color::Light(BaseColor::Yellow))
}

fn cyan() -> Style {
    Style::from(Color::Light(BaseColor::Cyan))
}

fn bold() -> Style {
    Style {
        effects: EnumSet::only(Effect::Bold).into(),
        color: ColorStyle::inherit_parent(),
    }
}

fn dim() -> Style {
    Style::from(Color::Dark(BaseColor::White))
}

// ── Report renderer ───────────────────────────────────────────────────────────

fn render_pytest_report(data: &ReportData) -> StyledString {
    let mut s = StyledString::new();

    // ── Title ────────────────────────────────────────────────────────────────
    let title = if data.title.is_empty() {
        "Test Report".to_string()
    } else {
        data.title.clone()
    };
    s.append_styled(format!("{title}\n"), bold());

    // ── Environment key/values ───────────────────────────────────────────────
    if !data.environment.is_empty() {
        let mut env_keys: Vec<&String> = data.environment.keys().collect();
        env_keys.sort();
        for k in env_keys {
            let v = &data.environment[k];
            let val = match v {
                serde_json::Value::String(st) => st.clone(),
                other => other.to_string(),
            };
            s.append_styled(format!("{k}: "), bold());
            s.append_styled(format!("{val}\n"), dim());
        }
    }
    s.append_plain("\n");

    // ── Collect and categorise test results ──────────────────────────────────
    let mut failed: Vec<&TestEntry> = Vec::new();
    let mut errors: Vec<&TestEntry> = Vec::new();
    let mut skipped: Vec<&TestEntry> = Vec::new();
    let mut passed: Vec<&TestEntry> = Vec::new();

    // Each key maps to a list; take the last (most recent) run per test.
    let mut entries: Vec<&TestEntry> = data.tests.values().filter_map(|runs| runs.last()).collect();
    entries.sort_by(|a, b| a.test_id.cmp(&b.test_id));

    for e in &entries {
        match e.result.to_lowercase().as_str() {
            "failed" | "failure" => failed.push(e),
            "error" => errors.push(e),
            "skipped" | "xfailed" | "xpassed" => skipped.push(e),
            _ => passed.push(e),
        }
    }

    // ── Summary line ─────────────────────────────────────────────────────────
    let sep = "\u{2500}".repeat(60);
    s.append_styled("Summary: ", bold());
    if !failed.is_empty() {
        s.append_styled(format!("{} Failed", failed.len()), red());
        s.append_plain("  ");
    }
    if !errors.is_empty() {
        s.append_styled(format!("{} Error", errors.len()), red());
        s.append_plain("  ");
    }
    if !passed.is_empty() {
        s.append_styled(format!("{} Passed", passed.len()), green());
        s.append_plain("  ");
    }
    if !skipped.is_empty() {
        s.append_styled(format!("{} Skipped", skipped.len()), yellow());
    }
    s.append_plain("\n");
    s.append_styled(format!("{sep}\n"), dim());

    // ── Failed ───────────────────────────────────────────────────────────────
    if !failed.is_empty() {
        s.append_styled("Failed:\n", red());
        for e in &failed {
            let short = short_test_id(&e.test_id);
            s.append_styled("  ✗ ", red());
            s.append_plain(format!("{short}  "));
            s.append_styled(format!("({})\n", e.duration), dim());
        }
        s.append_plain("\n");
    }

    // ── Errors ───────────────────────────────────────────────────────────────
    if !errors.is_empty() {
        s.append_styled("Errors:\n", red());
        for e in &errors {
            let short = short_test_id(&e.test_id);
            s.append_styled("  ✗ ", red());
            s.append_plain(format!("{short}  "));
            s.append_styled(format!("({})\n", e.duration), dim());
        }
        s.append_plain("\n");
    }

    // ── Skipped ──────────────────────────────────────────────────────────────
    if !skipped.is_empty() {
        s.append_styled("Skipped:\n", yellow());
        for e in &skipped {
            let short = short_test_id(&e.test_id);
            s.append_styled("  – ", yellow());
            s.append_plain(format!("{short}  "));
            s.append_styled(format!("({})\n", e.duration), dim());
        }
        s.append_plain("\n");
    }

    // ── Passed ───────────────────────────────────────────────────────────────
    if !passed.is_empty() {
        s.append_styled("Passed:\n", green());
        for e in &passed {
            let short = short_test_id(&e.test_id);
            s.append_styled("  ✓ ", green());
            s.append_plain(format!("{short}  "));
            s.append_styled(format!("({})\n", e.duration), dim());
        }
    }

    // ── CI log hint ──────────────────────────────────────────────────────────
    s.append_plain("\n");
    s.append_styled(format!("{sep}\n"), dim());
    s.append_styled("Select ", dim());
    s.append_styled("runner.log.gz", cyan());
    s.append_styled(" in the file list for the full job log.\n", dim());

    s
}

/// Strip the module path prefix, keeping only the class::method portion.
/// `test_integration/test_commands.py::TestIPACommand::test_foo` → `TestIPACommand::test_foo`
fn short_test_id(id: &str) -> &str {
    // Find the second "::" and return from the preceding component onwards.
    if let Some(pos) = id.rfind("::") {
        // Walk back to find the class name before this method.
        let before = &id[..pos];
        if let Some(pos2) = before.rfind("::") {
            return &id[pos2 + 2..];
        }
    }
    id
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_unescape() {
        assert_eq!(html_unescape("a &amp; b"), "a & b");
        assert_eq!(html_unescape("&quot;hi&quot;"), "\"hi\"");
        assert_eq!(html_unescape("&#34;hi&#34;"), "\"hi\"");
        assert_eq!(html_unescape("&lt;tag&gt;"), "<tag>");
        assert_eq!(html_unescape("no entities"), "no entities");
    }

    #[test]
    fn test_short_test_id_full_path() {
        assert_eq!(
            short_test_id("test_integration/test_commands.py::TestIPACommand::test_foo"),
            "TestIPACommand::test_foo"
        );
    }

    #[test]
    fn test_short_test_id_no_class() {
        assert_eq!(
            short_test_id("test_commands.py::test_foo"),
            "test_commands.py::test_foo"
        );
    }

    #[test]
    fn test_short_test_id_plain() {
        assert_eq!(short_test_id("test_foo"), "test_foo");
    }

    #[test]
    fn test_parse_directory_listing_folders_and_files() {
        let html = r#"
            <td><i class="fas fa-caret-up text-center"><a href="../"> Parent Directory </a></i></td>
            <td><i class="fas fa-folder"><a href="http://example.com/jobs/abc/logs/"> logs </a></i></td>
            <td><i class="fas fa-file"><a href="http://example.com/jobs/abc/report.html"> report.html </a></i></td>
            <td><i class="fas fa-file"><a href="http://example.com/jobs/abc/runner.log.gz"> runner.log.gz </a></i></td>
        "#;
        let entries = parse_directory_listing(html);
        assert_eq!(entries.len(), 3);
        // Dirs first
        assert_eq!(entries[0].name, "logs");
        assert!(entries[0].is_dir);
        // Files sorted alphabetically
        assert_eq!(entries[1].name, "report.html");
        assert!(!entries[1].is_dir);
        assert_eq!(entries[2].name, "runner.log.gz");
        assert!(!entries[2].is_dir);
    }

    #[test]
    fn test_parse_directory_listing_empty() {
        let entries = parse_directory_listing("<html><body></body></html>");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_can_handle() {
        let v = PrciViewer::new();
        assert!(
            v.can_handle("http://freeipa-org-pr-ci.s3-website.eu-central-1.amazonaws.com/jobs/abc")
        );
        assert!(!v.can_handle("https://github.com/freeipa/freeipa/pull/1"));
        assert!(!v.can_handle("https://dev.azure.com/something/"));
    }

    #[test]
    fn test_render_pytest_report_summary() {
        let data = ReportData {
            title: "Test Report".into(),
            environment: HashMap::new(),
            tests: {
                let mut m = HashMap::new();
                m.insert(
                    "mod::Class::test_pass".into(),
                    vec![TestEntry {
                        test_id: "mod::Class::test_pass".into(),
                        result: "Passed".into(),
                        duration: "00:00:01".into(),
                    }],
                );
                m.insert(
                    "mod::Class::test_fail".into(),
                    vec![TestEntry {
                        test_id: "mod::Class::test_fail".into(),
                        result: "Failed".into(),
                        duration: "00:00:02".into(),
                    }],
                );
                m
            },
        };
        let rendered = render_pytest_report(&data);
        let plain: String = rendered
            .spans()
            .map(|sp| sp.content.to_owned())
            .collect::<Vec<_>>()
            .join("");
        assert!(plain.contains("1 Failed"));
        assert!(plain.contains("1 Passed"));
        assert!(plain.contains("Class::test_fail"));
        assert!(plain.contains("Class::test_pass"));
    }
}
