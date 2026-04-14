//! Custom Markdown → [`StyledString`] renderer for the TUI detail pane.
//!
//! Extends the built-in cursive renderer with proper handling of:
//! - **Block quotes**: every line prefixed with `│ ` in yellow; nested
//!   blockquotes accumulate multiple `│ ` prefixes.
//! - **Fenced / indented code blocks**: code text indented and rendered in
//!   cyan so it visually stands out from surrounding prose.
//! - **Inline code**: rendered in cyan.
//! - **Bold / italic** (and combined bold-italic): passed through.
//! - **Links**: link text is shown normally; the URL is appended in blue
//!   inside parentheses.
//! - **Bullet and ordered lists**: items prefixed with `  •`.
//! - **Headings**: rendered bold.
//! - **Horizontal rules**: drawn with box-drawing characters.

use cursive::{
    reexports::enumset::EnumSet,
    theme::{BaseColor, Color, ColorStyle, Effect, Style},
    utils::markup::StyledString,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

// ─── Style helpers ────────────────────────────────────────────────────────────

fn bq_style() -> Style {
    Style::from(ColorStyle::front(Color::Dark(BaseColor::Yellow)))
}

fn code_style() -> Style {
    Style::from(ColorStyle::front(Color::Dark(BaseColor::Cyan)))
}

fn link_style() -> Style {
    Style::from(ColorStyle::front(Color::Dark(BaseColor::Blue)))
}

fn plain_style() -> Style {
    Style::from(ColorStyle::terminal_default())
}

fn build_style(in_code_block: bool, in_heading: bool, in_strong: bool, in_em: bool) -> Style {
    if in_code_block {
        return code_style();
    }
    if in_heading || (in_strong && !in_em) {
        return Style {
            color: ColorStyle::terminal_default(),
            effects: EnumSet::only(Effect::Bold).into(),
        };
    }
    if in_em && !in_strong {
        return Style {
            color: ColorStyle::terminal_default(),
            effects: EnumSet::only(Effect::Italic).into(),
        };
    }
    if in_strong && in_em {
        return Style {
            color: ColorStyle::terminal_default(),
            effects: (EnumSet::only(Effect::Bold) | EnumSet::only(Effect::Italic)).into(),
        };
    }
    plain_style()
}

// ─── Blockquote prefix helpers ────────────────────────────────────────────────

fn append_bq_prefix(out: &mut StyledString, depth: usize) {
    for _ in 0..depth {
        out.append_styled("│ ", bq_style());
    }
}

// ─── Public entry points ──────────────────────────────────────────────────────

/// Parse `input` as CommonMark and return a [`StyledString`] suitable for a
/// cursive [`TextView`](cursive::views::TextView).
pub fn render(input: &str) -> StyledString {
    let mut out = StyledString::new();

    let mut blockquote_depth: usize = 0;
    let mut in_code_block = false;
    let mut in_strong = false;
    let mut in_em = false;
    let mut in_heading = false;
    let mut link_url: Option<String> = None;
    // Whether the next text/code event inside a blockquote should be preceded
    // by a blockquote prefix (set when entering a new blockquote line).
    let mut need_bq_prefix = false;

    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(input, options);

    for event in parser {
        match event {
            // ── Block quotes ──────────────────────────────────────────────────
            Event::Start(Tag::BlockQuote(_)) => {
                blockquote_depth += 1;
                need_bq_prefix = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                out.append_plain("\n");
                blockquote_depth = blockquote_depth.saturating_sub(1);
                need_bq_prefix = blockquote_depth > 0;
            }

            // ── Code blocks ───────────────────────────────────────────────────
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                // Ensure we are on a fresh line before the indented block.
                if !out.source().is_empty() && !out.source().ends_with('\n') {
                    out.append_plain("\n");
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                out.append_plain("\n");
            }

            // ── Paragraphs ────────────────────────────────────────────────────
            Event::Start(Tag::Paragraph) => {
                // Separate paragraphs with a blank line.
                if !out.source().is_empty() {
                    if !out.source().ends_with('\n') {
                        out.append_plain("\n");
                    }
                    out.append_plain("\n");
                }
                if blockquote_depth > 0 && need_bq_prefix {
                    append_bq_prefix(&mut out, blockquote_depth);
                    need_bq_prefix = false;
                }
            }
            Event::End(TagEnd::Paragraph) => {
                // Let the next Start(Paragraph) add the separator.
            }

            // ── Headings ──────────────────────────────────────────────────────
            Event::Start(Tag::Heading { .. }) => {
                in_heading = true;
                if !out.source().is_empty() {
                    if !out.source().ends_with('\n') {
                        out.append_plain("\n");
                    }
                    out.append_plain("\n");
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
                out.append_plain("\n");
            }

            // ── Inline emphasis ───────────────────────────────────────────────
            Event::Start(Tag::Strong) => in_strong = true,
            Event::End(TagEnd::Strong) => in_strong = false,
            Event::Start(Tag::Emphasis) => in_em = true,
            Event::End(TagEnd::Emphasis) => in_em = false,

            // ── Links ─────────────────────────────────────────────────────────
            Event::Start(Tag::Link { dest_url, .. }) => {
                link_url = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                if let Some(url) = link_url.take() {
                    out.append_styled(format!(" ({})", url), link_style());
                }
            }

            // ── Lists ─────────────────────────────────────────────────────────
            Event::Start(Tag::List(_)) => {
                if !out.source().is_empty() && !out.source().ends_with('\n') {
                    out.append_plain("\n");
                }
            }
            Event::End(TagEnd::List(_)) => {
                out.append_plain("\n");
            }
            Event::Start(Tag::Item) => {
                if blockquote_depth > 0 {
                    append_bq_prefix(&mut out, blockquote_depth);
                }
                out.append_plain("  \u{2022} ");
            }
            Event::End(TagEnd::Item) => {
                if !out.source().ends_with('\n') {
                    out.append_plain("\n");
                }
            }

            // ── Text ──────────────────────────────────────────────────────────
            Event::Text(text) => {
                if blockquote_depth > 0 && need_bq_prefix {
                    append_bq_prefix(&mut out, blockquote_depth);
                    need_bq_prefix = false;
                }
                let style = build_style(in_code_block, in_heading, in_strong, in_em);
                let s = text.as_ref();
                if in_code_block {
                    // Indent every line of the code block in cyan.
                    let mut first = true;
                    for line in s.split('\n') {
                        if !first {
                            out.append_plain("\n");
                        }
                        out.append_styled(format!("  {}", line), style);
                        first = false;
                    }
                } else if blockquote_depth > 0 {
                    // Within a blockquote, re-prefix after each embedded newline.
                    let mut first = true;
                    for line in s.split('\n') {
                        if !first {
                            out.append_plain("\n");
                            append_bq_prefix(&mut out, blockquote_depth);
                        }
                        out.append_styled(line, style);
                        first = false;
                    }
                } else {
                    out.append_styled(s, style);
                }
            }

            // ── Inline code ───────────────────────────────────────────────────
            Event::Code(text) => {
                if blockquote_depth > 0 && need_bq_prefix {
                    append_bq_prefix(&mut out, blockquote_depth);
                    need_bq_prefix = false;
                }
                out.append_styled(text.as_ref(), code_style());
            }

            // ── Line breaks ───────────────────────────────────────────────────
            Event::SoftBreak => {
                out.append_plain("\n");
                if blockquote_depth > 0 {
                    append_bq_prefix(&mut out, blockquote_depth);
                }
            }
            Event::HardBreak => {
                out.append_plain("\n");
                if blockquote_depth > 0 {
                    append_bq_prefix(&mut out, blockquote_depth);
                }
            }

            // ── Horizontal rule ───────────────────────────────────────────────
            Event::Rule => {
                out.append_plain("\n\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n");
            }

            _ => {}
        }
    }

    out
}

/// Like [`render`] but prefixes every rendered line with `indent`.
///
/// Use this to display a comment body indented relative to its header without
/// touching the rendering logic.
pub fn render_indented(input: &str, indent: &str) -> StyledString {
    let rendered = render(input);
    if indent.is_empty() {
        return rendered;
    }
    let mut out = StyledString::new();
    // Emit the indent at the very start of the first line.
    out.append_plain(indent);
    for span in rendered.spans() {
        let text = span.content;
        let style = *span.attr;
        // Split each span's text at newlines; re-emit the indent prefix after
        // every newline so that every visual line begins with `indent`.
        let parts: Vec<&str> = text.split('\n').collect();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                out.append_plain("\n");
                out.append_plain(indent);
            }
            if !part.is_empty() {
                out.append_styled(*part, style);
            }
        }
    }
    out
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &StyledString) -> &str {
        s.source()
    }

    #[test]
    fn test_plain_paragraph() {
        let out = render("Hello world");
        assert!(plain(&out).contains("Hello world"));
    }

    #[test]
    fn test_inline_code() {
        let out = render("Use `foo()` here");
        // The raw text includes the content; the style is cyan but source has it.
        assert!(plain(&out).contains("foo()"));
    }

    #[test]
    fn test_code_block() {
        let out = render("```\nlet x = 1;\n```");
        let src = plain(&out);
        assert!(src.contains("let x = 1;"));
    }

    #[test]
    fn test_blockquote_prefix() {
        let out = render("> quoted line");
        let src = plain(&out);
        // The │ prefix and the quoted text should both appear.
        assert!(src.contains('│'), "expected blockquote prefix in: {}", src);
        assert!(src.contains("quoted line"), "expected text in: {}", src);
    }

    #[test]
    fn test_nested_blockquote() {
        let out = render("> outer\n> > inner");
        let src = plain(&out);
        assert!(src.contains('│'));
        assert!(src.contains("outer"));
        assert!(src.contains("inner"));
    }

    #[test]
    fn test_bullet_list() {
        let out = render("- alpha\n- beta");
        let src = plain(&out);
        assert!(src.contains('\u{2022}'), "expected bullet in: {}", src);
        assert!(src.contains("alpha"));
        assert!(src.contains("beta"));
    }

    #[test]
    fn test_horizontal_rule() {
        let out = render("---");
        let src = plain(&out);
        assert!(src.contains('\u{2500}'));
    }

    #[test]
    fn test_link_url_appended() {
        let out = render("[click here](https://example.com)");
        let src = plain(&out);
        assert!(src.contains("click here"));
        assert!(src.contains("https://example.com"));
    }

    #[test]
    fn test_heading_rendered() {
        let out = render("# Big heading");
        let src = plain(&out);
        assert!(src.contains("Big heading"));
    }

    #[test]
    fn test_empty_input() {
        let out = render("");
        assert!(plain(&out).is_empty());
    }

    #[test]
    fn test_two_paragraphs_separated() {
        let out = render("First paragraph.\n\nSecond paragraph.");
        let src = plain(&out);
        assert!(src.contains("First paragraph."));
        assert!(src.contains("Second paragraph."));
    }

    #[test]
    fn test_render_indented_prefixes_every_line() {
        let out = render_indented("Line one.\n\nLine two.", "  ");
        let src = plain(&out);
        // Every line should start with two spaces.
        for line in src.lines() {
            if !line.is_empty() {
                assert!(
                    line.starts_with("  "),
                    "expected indent on line: {:?}",
                    line
                );
            }
        }
        assert!(src.contains("Line one."));
        assert!(src.contains("Line two."));
    }

    #[test]
    fn test_render_indented_empty_indent_is_noop() {
        let a = render("Hello world");
        let b = render_indented("Hello world", "");
        assert_eq!(plain(&a), plain(&b));
    }
}
