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

use super::{ArtifactEntry, CiJobViewer, ContentResult};

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

    fn fetch_content(&self, entry: &ArtifactEntry) -> Result<ContentResult> {
        if entry.name.ends_with(".gz") {
            let bytes = self.fetch_bytes(&entry.url)?;
            let mut decoder = GzDecoder::new(&bytes[..]);
            let mut text = String::new();
            decoder
                .read_to_string(&mut text)
                .context("decompressing gzip content")?;
            return Ok(ContentResult {
                content: StyledString::plain(text),
                sub_entries: vec![],
            });
        }

        let text = self.fetch_text(&entry.url)?;

        if entry.name == "report.html" || entry.name.ends_with(".html") {
            if let Some((content, sub_entries)) = try_parse_pytest_report(&text, &entry.url) {
                return Ok(ContentResult {
                    content,
                    sub_entries,
                });
            }
            // Couldn't extract the JSON blob — raw HTML is unreadable in a terminal.
            return Ok(ContentResult {
                content: StyledString::plain(
                    "[Could not parse as a pytest HTML report.\n\
                     The file may use an unsupported format.\n\
                     Open it in a browser for the full view.]\n",
                ),
                sub_entries: vec![],
            });
        }

        Ok(ContentResult {
            content: StyledString::plain(text),
            sub_entries: vec![],
        })
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
    /// Log output captured during the test (HTML-entity-encoded by pytest-html).
    #[serde(default)]
    log: String,
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

/// Attempt to parse a pytest HTML report.
///
/// Returns `Some((summary, sub_entries))` on success where:
/// - `summary` is a `StyledString` overview for the right pane.
/// - `sub_entries` is one `(ArtifactEntry, StyledString)` per test case whose
///   log content is pre-rendered; the `ArtifactEntry` URL is a synthetic key
///   of the form `{base_url}#test/{test_id}` used only as a cache key.
///
/// Returns `None` if the expected JSON blob is not found (not a pytest report).
fn try_parse_pytest_report(
    html: &str,
    base_url: &str,
) -> Option<(StyledString, Vec<(ArtifactEntry, StyledString)>)> {
    // The JSON blob lives in:  <div id="data-container" data-jsonblob="...">
    let re =
        Regex::new(r#"id="data-container"[^>]*data-jsonblob="([^"]+)""#).expect("static regex");
    let cap = re.captures(html)?;
    let raw = html_unescape(&cap[1]);
    let data: ReportData = serde_json::from_str(&raw).ok()?;

    let summary = render_pytest_report(&data);

    // Build sub-entries: one per test case, sorted by test ID.
    let mut entries: Vec<&TestEntry> = data
        .tests
        .values()
        .filter_map(|runs| runs.last())
        .collect();
    entries.sort_by(|a, b| a.test_id.cmp(&b.test_id));

    let sub_entries: Vec<(ArtifactEntry, StyledString)> = entries
        .into_iter()
        .map(|e| {
            let icon = result_icon(&e.result);
            let short = short_test_id(&e.test_id).to_string();
            let artifact = ArtifactEntry {
                name: format!("{} {}", icon, short),
                url: format!("{}#test/{}", base_url, e.test_id),
                is_dir: false,
            };
            let log_content = render_test_log(e);
            (artifact, log_content)
        })
        .collect();

    Some((summary, sub_entries))
}

/// Minimal HTML entity unescaping sufficient for the JSON blob.
///
/// pytest-html 4.x uses `&#34;` for structural JSON quotes (e.g. string
/// delimiters) and `&quot;` for literal `"` characters inside string values
/// (e.g. from shell command output like `ss` showing `"systemd-resolve"`).
/// We must NOT expand `&quot;` → `"` because those embedded quotes are not
/// JSON-escaped, and doing so would produce invalid JSON.  Leaving `&quot;`
/// as-is in non-parsed fields (like `log`) is harmless.
fn html_unescape(s: &str) -> String {
    s.replace("&#34;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Secondary unescape for log field content.
///
/// After the primary JSON-blob unescape the `log` field still contains
/// HTML entities from pytest-html's double-encoding of the log text.
/// This pass converts them to display characters.  `&quot;` IS safe to
/// expand here because we are rendering plain text, not parsing JSON.
///
/// `&amp;` must come first so that `&amp;quot;` (double-encoded `"`) is
/// reduced to `&quot;` before the `&quot;` → `"` step fires.
fn log_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
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

fn result_style(result: &str) -> Style {
    match result.to_lowercase().as_str() {
        "failed" | "failure" | "error" => red(),
        "skipped" | "xfailed" | "xpassed" => yellow(),
        _ => green(),
    }
}

fn result_icon(result: &str) -> &'static str {
    match result.to_lowercase().as_str() {
        "failed" | "failure" | "error" => "✗",
        "skipped" | "xfailed" | "xpassed" => "–",
        _ => "✓",
    }
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
    let mut entries: Vec<&TestEntry> = data
        .tests
        .values()
        .filter_map(|runs| runs.last())
        .collect();
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

    // ── Hint ─────────────────────────────────────────────────────────────────
    s.append_plain("\n");
    s.append_styled(format!("{sep}\n"), dim());
    s.append_styled("Test logs: select a test entry in the file list.\n", dim());
    s.append_styled("Select ", dim());
    s.append_styled("runner.log.gz", cyan());
    s.append_styled(" for the full job log.\n", dim());

    s
}

/// Render a single test's log for display in the right pane.
fn render_test_log(entry: &TestEntry) -> StyledString {
    let mut s = StyledString::new();

    let icon = result_icon(&entry.result);
    let style = result_style(&entry.result);

    s.append_styled(format!("{} {}\n", icon, entry.test_id), bold());
    s.append_styled(
        format!("Result: {}  Duration: {}\n", entry.result, entry.duration),
        style,
    );
    s.append_styled("\u{2500}".repeat(60) + "\n", dim());

    let log = log_unescape(&entry.log);
    if log.trim().is_empty() {
        s.append_styled("(no log output captured)\n", dim());
    } else {
        s.append_plain(log);
    }

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
        // &#34; is the structural JSON quote used by pytest-html 4.x — must expand.
        assert_eq!(html_unescape("&#34;hi&#34;"), "\"hi\"");
        // &quot; inside log content strings must NOT expand: the embedded " is not
        // JSON-escaped, so expanding it would produce invalid JSON.
        assert_eq!(html_unescape("&quot;hi&quot;"), "&quot;hi&quot;");
        assert_eq!(html_unescape("&lt;tag&gt;"), "<tag>");
        assert_eq!(html_unescape("no entities"), "no entities");
        // &amp;quot; is a double-encoded literal quote in log content.
        // Old code: &amp; → & first, then &quot; → " → breaks JSON.
        // New code: &amp; is last, so &amp;quot; → &quot; (harmless string text).
        assert_eq!(html_unescape("a&amp;quot;b"), "a&quot;b");
    }

    #[test]
    fn test_log_unescape() {
        // In log content, &quot; IS safe to expand (it's display text, not JSON).
        assert_eq!(log_unescape("a&quot;b"), "a\"b");
        assert_eq!(log_unescape("&#x27;hello&#x27;"), "'hello'");
        assert_eq!(log_unescape("&amp;"), "&");
        // &amp; must be last so &amp;quot; → &quot; → "
        assert_eq!(log_unescape("&amp;quot;"), "\"");
    }

    /// Verify that a pytest-html 4.x JSON blob containing double-encoded
    /// quotes (`&amp;quot;`) in the log field parses successfully.
    #[test]
    fn test_try_parse_pytest_report_with_double_encoded_quotes() {
        let log_value = r#"ESTAB users:((&amp;quot;systemd-resolve&amp;quot;,pid=608,fd=27))"#;
        let html = format!(
            r#"<div id="data-container" data-jsonblob="{{&#34;title&#34;: &#34;Test Report&#34;, &#34;environment&#34;: {{}}, &#34;tests&#34;: {{&#34;mod::Class::test_pass&#34;: [{{&#34;testId&#34;: &#34;mod::Class::test_pass&#34;, &#34;result&#34;: &#34;Passed&#34;, &#34;duration&#34;: &#34;00:00:01&#34;, &#34;log&#34;: &#34;{}&#34;}}]}}}}"></div>"#,
            log_value
        );

        let result = try_parse_pytest_report(&html, "http://example.com/report.html");
        assert!(
            result.is_some(),
            "Parsing failed — double-encoded quotes in log broke JSON"
        );
        let (summary, sub_entries) = result.unwrap();
        let plain: String = summary
            .spans()
            .map(|sp| sp.content.to_owned())
            .collect::<Vec<_>>()
            .join("");
        assert!(plain.contains("1 Passed"));
        assert_eq!(sub_entries.len(), 1);
        assert!(sub_entries[0]
            .0
            .url
            .ends_with("#test/mod::Class::test_pass"));
        // The log should have the double-encoded quote fully unescaped for display.
        let log_plain: String = sub_entries[0]
            .1
            .spans()
            .map(|sp| sp.content.to_owned())
            .collect::<Vec<_>>()
            .join("");
        assert!(log_plain.contains("systemd-resolve"));
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
                        log: String::new(),
                    }],
                );
                m.insert(
                    "mod::Class::test_fail".into(),
                    vec![TestEntry {
                        test_id: "mod::Class::test_fail".into(),
                        result: "Failed".into(),
                        duration: "00:00:02".into(),
                        log: String::new(),
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
