use anyhow::Result;
use cursive::{
    event::{EventResult, Key},
    reexports::enumset::EnumSet,
    theme::{BaseColor, Color, ColorStyle, Effect, Style},
    traits::*,
    utils::markup::StyledString,
    view::scroll::Scroller,
    views::{
        BoxedView, Checkbox, Dialog, EditView, LinearLayout, NamedView, OnEventView, Panel,
        ScrollView, SelectView, TextArea, TextView,
    },
    Cursive,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use super::pr_client::PrClient;
use super::Ctx;
use crate::api::types::{CiStatus, IssueComment, Label, PrFile, PullRequest, ReviewComment};
use crate::api::pagure::PagureClient;
use crate::tui_keys::TuiKeys;

const FORGEJO_OFFLINE_MSG: &str =
    "Offline mode is not yet supported for Forgejo. Connect to the network and try again.";

// ─── Persistent TUI state (survives layer pops on resize) ─────────────────────

/// Snapshot of the current review session, stored so the layer can be
/// rebuilt after the comment form is closed.
#[derive(Clone)]
struct ReviewSession {
    pr: PullRequest,
    items: Vec<ReviewItem>,
    commit_id: String,
}

/// Action set by a TUI form and executed in the terminal after `siv.run()` returns.
#[derive(Clone)]
enum PendingTuiAction {
    PrPush {
        pr_id: u64,
        reviewers: Vec<String>,
        backport_branches: Vec<String>,
        autobackport: bool,
    },
    Backport {
        pr_id: u64,
        branches: Vec<String>,
    },
}

#[derive(Clone)]
struct TuiState {
    pr_client: Option<Arc<PrClient>>,
    prs: Option<Vec<PullRequest>>,
    review: Option<ReviewSession>,
    offline: bool,
    db: Option<Arc<crate::db::Database>>,
    pending_action: Option<PendingTuiAction>,
    /// The PR state filter in use ("open", "closed", "all").
    state: String,
    /// Active profile name (empty string for the default profile).
    /// Used as the cache namespace so different profiles don't share cached PR data.
    profile: String,
    tui_keys: TuiKeys,
    /// Whether keyboard focus is on the right (detail) pane.
    focus_right: bool,
    /// CI job statuses for the currently selected PR (populated by background fetch).
    /// Stored here so the 'i' key handler can open the job selector without re-fetching.
    ci_statuses: HashMap<String, CiStatus>,
    /// Bug tracker client for filing issues from the CI viewer.
    pagure: Option<Arc<PagureClient>>,
}

// ─── Entry point ─────────────────────────────────────────────────────────────

/// Outer loop: run the TUI, then execute any pending terminal action (push /
/// backport), then re-enter the TUI so the user sees the updated state.
pub fn run(ctx: &mut Ctx, state: &str) -> Result<()> {
    loop {
        match run_tui_once(ctx, state)? {
            None => return Ok(()),
            Some(PendingTuiAction::PrPush {
                pr_id,
                reviewers,
                backport_branches,
                autobackport,
            }) => {
                if let Err(e) =
                    super::pr_push::run(ctx, pr_id, &reviewers, &backport_branches, autobackport)
                {
                    eprintln!("\x1b[31mPush failed: {:#}\x1b[0m", e);
                }
                crate::output::prompt("Press Enter to return to the TUI…");
            }
            Some(PendingTuiAction::Backport { pr_id, branches }) => {
                if let Err(e) = super::backport::run_backport_cmd(ctx, pr_id, &branches) {
                    eprintln!("\x1b[31mBackport failed: {:#}\x1b[0m", e);
                }
                crate::output::prompt("Press Enter to return to the TUI…");
            }
        }
    }
}

/// Single TUI session.  Returns `None` on normal quit, or the pending action
/// the user triggered so the caller can execute it in the terminal.
fn run_tui_once(ctx: &Ctx, state: &str) -> Result<Option<PendingTuiAction>> {
    let gh = ctx.pr_client_or_err()?.clone();

    let offline = ctx.offline;
    let db = ctx.db.clone();

    let mut siv = cursive::crossterm();
    ctx.tui_style.apply(&mut siv);

    // Initialise user_data so the resize callback can always read it.
    siv.set_user_data(TuiState {
        pr_client: None,
        prs: None,
        review: None,
        offline,
        db: db.clone(),
        pending_action: None,
        state: state.to_string(),
        profile: ctx.profile.clone(),
        tui_keys: ctx.tui_keys.clone(),
        focus_right: false,
        ci_statuses: HashMap::new(),
        pagure: ctx.pagure.clone(),
    });

    siv.add_global_callback(ctx.tui_keys.quit, |s| s.quit());

    // Rebuild the pane layout whenever the terminal is resized.
    siv.add_global_callback(cursive::event::Event::WindowResize, |s| {
        let gh = s.user_data::<TuiState>().and_then(|t| t.pr_client.clone());
        let prs = s.user_data::<TuiState>().and_then(|t| t.prs.clone());
        if let (Some(gh), Some(prs)) = (gh, prs) {
            // Close every layer (dialogs + main layout) so we can rebuild clean.
            while s.pop_layer().is_some() {}
            build_two_pane(s, gh, prs);
        }
        // If prs haven't been loaded yet the loading dialog is still showing;
        // cursive will redraw it at the new size automatically.
    });

    let state = state.to_string();
    let profile = ctx.profile.clone();
    let gh2 = Arc::clone(&gh);

    // In online mode, if the cache already holds PRs for the requested state,
    // display them immediately so the TUI is interactive right away.  A silent
    // background thread then fetches fresh data and swaps the list in once it
    // arrives.  On network error the cached view remains untouched.
    let cached_prs = if !offline {
        db.as_ref()
            .and_then(|d| d.load_prs(&profile, &state).ok())
            .filter(|v| !v.is_empty())
    } else {
        None
    };

    if let Some(initial_prs) = cached_prs {
        build_two_pane(&mut siv, Arc::clone(&gh), initial_prs);
        let cb = siv.cb_sink().clone();
        let db2 = db.clone();
        let profile2 = profile.clone();
        std::thread::spawn(move || match gh2.list_prs(&state) {
            Ok(prs) => {
                if let Some(ref d) = db2 {
                    if let Err(e) = d.cache_prs(&profile2, &prs) {
                        eprintln!("Warning: failed to update PR cache: {}", e);
                    }
                }
                let gh3 = gh2;
                cb.send(Box::new(move |s: &mut Cursive| {
                    while s.pop_layer().is_some() {}
                    build_two_pane(s, gh3, prs);
                }))
                .ok();
            }
            Err(e) => {
                eprintln!(
                    "Warning: background PR list refresh failed: {}. Keeping cached view.",
                    e
                );
            }
        });
    } else {
        // No cached data: show a blocking loading dialog until the fetch lands.
        siv.add_layer(loading_dialog("Fetching pull requests…"));
        let cb = siv.cb_sink().clone();
        std::thread::spawn(move || {
            let result: anyhow::Result<Vec<_>> = (|| {
                if offline {
                    match &db {
                        Some(db) => {
                            let prs = db.load_prs(&profile, &state)?;
                            if prs.is_empty() {
                                Err(anyhow::anyhow!("No cached data available in offline mode."))
                            } else {
                                Ok(prs)
                            }
                        }
                        None => Err(anyhow::anyhow!("No cached data available in offline mode.")),
                    }
                } else {
                    let prs = gh2.list_prs(&state)?;
                    if let Some(ref db) = db {
                        if let Err(e) = db.cache_prs(&profile, &prs) {
                            eprintln!("Warning: failed to cache PRs: {}", e);
                        }
                    }
                    Ok(prs)
                }
            })();
            let gh3 = gh2;
            cb.send(Box::new(move |s: &mut Cursive| {
                s.pop_layer();
                match result {
                    Err(e) => show_error(s, &format!("Failed to load PRs: {}", e)),
                    Ok(prs) if prs.is_empty() => show_error(s, "No pull requests found."),
                    Ok(prs) => build_two_pane(s, gh3, prs),
                }
            }))
            .ok();
        });
    }

    siv.run();
    Ok(siv
        .user_data::<TuiState>()
        .and_then(|t| t.pending_action.clone()))
}

// ─── Two-pane layout ─────────────────────────────────────────────────────────

/// Computes the left-pane width from the terminal width.
/// 40 % of the screen, clamped to [35, 60].
fn compute_left_width(screen_width: usize) -> usize {
    (screen_width * 2 / 5).clamp(35, 60)
}

fn build_two_pane(siv: &mut Cursive, gh: Arc<PrClient>, prs: Vec<PullRequest>) {
    // Preserve offline/db/state/profile/keys/focus/pagure from existing user_data (set during run()) or use defaults.
    let (offline, db, current_state, profile, tui_keys, focus_right, pagure) = siv
        .user_data::<TuiState>()
        .map(|t| {
            (
                t.offline,
                t.db.clone(),
                t.state.clone(),
                t.profile.clone(),
                t.tui_keys.clone(),
                t.focus_right,
                t.pagure.clone(),
            )
        })
        .unwrap_or_else(|| {
            (
                false,
                None,
                "open".to_string(),
                String::new(),
                TuiKeys::default(),
                false,
                None,
            )
        });

    // Persist the current dataset so the resize callback can rebuild.
    siv.set_user_data(TuiState {
        pr_client: Some(Arc::clone(&gh)),
        prs: Some(prs.clone()),
        review: None,
        offline,
        db,
        pending_action: None,
        state: current_state,
        profile,
        tui_keys: tui_keys.clone(),
        focus_right,
        ci_statuses: HashMap::new(),
        pagure,
    });

    let left_width = compute_left_width(siv.screen_size().x);
    // Inner width available inside the panel border (1 char each side).
    let inner_width = left_width.saturating_sub(2);

    // Pre-clone Arcs for each closure that needs one.
    let gh_select = Arc::clone(&gh);
    let gh_a = Arc::clone(&gh);
    let gh_x = Arc::clone(&gh);
    let gh_r = Arc::clone(&gh);
    let gh_c = Arc::clone(&gh);
    let gh_s = Arc::clone(&gh);
    let gh_enter = Arc::clone(&gh);

    let mut select = SelectView::<PullRequest>::new();
    for pr in &prs {
        select.add_item(pr_row_styled(pr, inner_width), pr.clone());
    }

    // Fires on Up/Down arrow keys and mouse clicks; keeps right pane in sync.
    select.set_on_select(move |s, pr: &PullRequest| {
        update_detail_from_pr(s, pr);
        fetch_pr_details_in_background(s, Arc::clone(&gh_select), pr.clone());
    });

    // Enter opens an action pop-up.
    select.set_on_submit(move |s, pr: &PullRequest| {
        show_action_dialog(s, Arc::clone(&gh_enter), pr.clone());
    });

    // Extract key copies (char: Copy) for use in the closure chain below.
    let key_ack = tui_keys.ack;
    let key_reject = tui_keys.reject;
    let key_browser = tui_keys.browser;
    let key_refresh = tui_keys.refresh;
    let key_review = tui_keys.review;
    let key_sync = tui_keys.sync;
    let key_down = tui_keys.down;
    let key_up = tui_keys.up;
    let key_scroll_down = tui_keys.scroll_down;
    let key_scroll_up = tui_keys.scroll_up;
    let key_inspect = tui_keys.inspect;

    let select_with_keys = OnEventView::new(ScrollView::new(select.with_name("pr_list")))
        .on_event(key_ack, move |s| {
            if let Some(pr) = selected_pr(s) {
                show_ack_dialog(s, Arc::clone(&gh_a), pr.number);
            }
        })
        .on_event(key_reject, move |s| {
            if let Some(pr) = selected_pr(s) {
                show_reject_dialog(s, Arc::clone(&gh_x), pr.number);
            }
        })
        .on_event(key_browser, |s| {
            if let Some(pr) = selected_pr(s) {
                open_browser(&pr.html_url);
            }
        })
        .on_event(key_refresh, move |s| {
            refresh_list(s, Arc::clone(&gh_r));
        })
        .on_event(key_review, move |s| {
            if let Some(pr) = selected_pr(s) {
                show_review_view(s, Arc::clone(&gh_c), pr);
            }
        })
        .on_event(key_sync, move |s| {
            let _ = &gh_s; // keep Arc alive
            sync_queued_actions(s);
        })
        // j: move PR selection (left focus) or scroll detail pane (right focus).
        // Uses on_event (AfterChild) because SelectView ignores 'j'.
        .on_event(key_down, |s| {
            nav_down(s);
        })
        // ↓: must use on_pre_event_inner (BeforeChild) because SelectView *consumes*
        // Key::Down for its own navigation, so on_event (AfterChild) would never fire.
        .on_pre_event_inner(Key::Down, |_, _| Some(EventResult::with_cb(nav_down)))
        // k: move PR selection (left focus) or scroll detail pane (right focus).
        .on_event(key_up, |s| {
            nav_up(s);
        })
        // ↑: same reasoning as ↓.
        .on_pre_event_inner(Key::Up, |_, _| Some(EventResult::with_cb(nav_up)))
        // d / u: scroll the detail pane 5 lines at a time regardless of pane focus.
        .on_event(key_scroll_down, |s| {
            scroll_detail(s, 5);
        })
        .on_event(key_scroll_up, |s| {
            scroll_detail(s, -5);
        })
        // Tab: toggle focus between the left (PR list) and right (detail) pane.
        .on_event(Key::Tab, |s| {
            if let Some(t) = s.user_data::<TuiState>() {
                t.focus_right = !t.focus_right;
            }
            update_help_bar(s);
        })
        // i: open CI job results viewer for the selected PR.
        .on_event(key_inspect, |s| {
            if let Some(pr) = selected_pr(s) {
                show_ci_job_selector(s, pr);
            }
        });

    let left_panel = Panel::new(BoxedView::boxed(select_with_keys.full_height()))
        .title(panel_title(&format!("{} PRs", prs.len()), !focus_right))
        .with_name("pr_left_panel")
        .fixed_width(left_width);

    let initial_detail = prs.first().map(pr_summary).unwrap_or_default();
    let right_panel = Panel::new(BoxedView::boxed(
        ScrollView::new(TextView::new(initial_detail).with_name("pr_detail"))
            .with_name("pr_detail_scroll")
            .full_screen(),
    ))
    .title(panel_title("Details", focus_right))
    .with_name("pr_right_panel")
    .full_width();

    let (offline_bar, queued) = siv
        .user_data::<TuiState>()
        .map(|t| {
            (
                t.offline,
                t.db.as_ref().map(|d| d.pending_count()).unwrap_or(0),
            )
        })
        .unwrap_or((false, 0));

    let help_styled = build_help_content(&tui_keys, focus_right, offline_bar, queued);
    let help = TextView::new(help_styled).with_name("help_bar");

    let layout = LinearLayout::vertical().child(help).child(
        LinearLayout::horizontal()
            .child(left_panel)
            .child(right_panel)
            .full_screen(),
    );

    siv.add_fullscreen_layer(layout);

    // Kick off detail fetch for the initially-focused PR.
    if let Some(pr) = prs.first() {
        fetch_pr_details_in_background(siv, gh, pr.clone());
    }
}

// ─── Help bar ─────────────────────────────────────────────────────────────────

/// Build a panel title as a `StyledString`, coloring it cyan when the panel has focus.
fn panel_title(text: &str, active: bool) -> StyledString {
    if active {
        StyledString::styled(text, Style::from(Color::Light(BaseColor::Cyan)))
    } else {
        StyledString::plain(text)
    }
}

/// Build the help-bar content reflecting the current pane focus and offline state.
fn build_help_content(
    tui_keys: &TuiKeys,
    focus_right: bool,
    offline: bool,
    queued: usize,
) -> StyledString {
    let keys = format!(
        "  {}:ACK  {}:Reject  {}:Review  {}:Browser  {}:Refresh  {}:Quit  \
         {}/↓:Down  {}/↑:Up  {}/{}:Scroll  Tab:Pane  {}:Inspect  Enter:Actions",
        tui_keys.ack,
        tui_keys.reject,
        tui_keys.review,
        tui_keys.browser,
        tui_keys.refresh,
        tui_keys.quit,
        tui_keys.down,
        tui_keys.up,
        tui_keys.scroll_down,
        tui_keys.scroll_up,
        tui_keys.inspect,
    );
    let mut s = StyledString::new();
    if offline {
        let offline_tag = format!("[OFFLINE | {} queued | {}:Sync]", queued, tui_keys.sync);
        s.append_styled(offline_tag, Style::from(Color::Light(BaseColor::Yellow)));
    }
    if focus_right {
        s.append_styled("[Detail] ", Style::from(Color::Light(BaseColor::Cyan)));
    }
    s.append_plain(format!(" {}", keys.trim_start()));
    s
}

/// Scroll the detail pane by `lines` (positive = down, negative = up).
fn scroll_detail(siv: &mut Cursive, lines: i32) {
    siv.call_on_name(
        "pr_detail_scroll",
        |v: &mut ScrollView<NamedView<TextView>>| {
            let cur = v.get_scroller().content_viewport().top() as i32;
            let next = (cur + lines).max(0) as usize;
            v.set_offset(cursive::Vec2::new(0, next));
        },
    );
}

/// Move selection down (left focus) or scroll detail pane down 1 line (right focus).
fn nav_down(siv: &mut Cursive) {
    let right = siv
        .user_data::<TuiState>()
        .map(|t| t.focus_right)
        .unwrap_or(false);
    if right {
        scroll_detail(siv, 1);
    } else if let Some(cb) =
        siv.call_on_name("pr_list", |v: &mut SelectView<PullRequest>| v.select_down(1))
    {
        cb(siv);
    }
}

/// Move selection up (left focus) or scroll detail pane up 1 line (right focus).
fn nav_up(siv: &mut Cursive) {
    let right = siv
        .user_data::<TuiState>()
        .map(|t| t.focus_right)
        .unwrap_or(false);
    if right {
        scroll_detail(siv, -1);
    } else if let Some(cb) =
        siv.call_on_name("pr_list", |v: &mut SelectView<PullRequest>| v.select_up(1))
    {
        cb(siv);
    }
}

/// Rebuild the help bar `TextView` to reflect the current `TuiState`.
fn update_help_bar(siv: &mut Cursive) {
    let data = siv.user_data::<TuiState>().map(|t| {
        (
            t.tui_keys.clone(),
            t.focus_right,
            t.offline,
            t.db.as_ref().map(|d| d.pending_count()).unwrap_or(0),
        )
    });
    if let Some((keys, focus_right, offline, queued)) = data {
        let content = build_help_content(&keys, focus_right, offline, queued);
        siv.call_on_name("help_bar", |v: &mut TextView| v.set_content(content));
        let pr_count = siv
            .call_on_name("pr_list", |v: &mut SelectView<PullRequest>| v.len())
            .unwrap_or(0);
        siv.call_on_name("pr_left_panel", |p: &mut Panel<BoxedView>| {
            p.set_title(panel_title(&format!("{} PRs", pr_count), !focus_right));
        });
        siv.call_on_name("pr_right_panel", |p: &mut Panel<BoxedView>| {
            p.set_title(panel_title("Details", focus_right));
        });
    }
}

// ─── Selection helper ─────────────────────────────────────────────────────────

fn selected_pr(siv: &mut Cursive) -> Option<PullRequest> {
    siv.call_on_name("pr_list", |v: &mut SelectView<PullRequest>| {
        v.selection().map(|rc| (*rc).clone())
    })
    .flatten()
}

// ─── Refresh ─────────────────────────────────────────────────────────────────

fn refresh_list(siv: &mut Cursive, gh: Arc<PrClient>) {
    let (offline, db, state, profile) = siv
        .user_data::<TuiState>()
        .map(|t| (t.offline, t.db.clone(), t.state.clone(), t.profile.clone()))
        .unwrap_or((false, None, "open".to_string(), String::new()));

    siv.pop_layer();
    siv.add_layer(loading_dialog("Refreshing pull requests…"));

    let cb = siv.cb_sink().clone();
    std::thread::spawn(move || {
        let result: anyhow::Result<Vec<_>> = (|| {
            if offline {
                match &db {
                    Some(db) => {
                        let prs = db.load_prs(&profile, &state)?;
                        if prs.is_empty() {
                            Err(anyhow::anyhow!("No cached data available in offline mode."))
                        } else {
                            Ok(prs)
                        }
                    }
                    None => Err(anyhow::anyhow!("No cached data available in offline mode.")),
                }
            } else {
                let prs = gh.list_prs(&state)?;
                if let Some(ref db) = db {
                    if let Err(e) = db.cache_prs(&profile, &prs) {
                        eprintln!("Warning: failed to cache PRs: {}", e);
                    }
                }
                Ok(prs)
            }
        })();
        let gh2 = gh;
        cb.send(Box::new(move |s: &mut Cursive| {
            s.pop_layer();
            match result {
                Err(e) => show_error(s, &format!("Refresh failed: {}", e)),
                Ok(prs) if prs.is_empty() => show_error(s, "No pull requests found."),
                Ok(prs) => build_two_pane(s, gh2, prs),
            }
        }))
        .ok();
    });
}

// ─── Detail pane content ─────────────────────────────────────────────────────

fn pr_label_names(pr: &PullRequest) -> Vec<String> {
    pr.labels.iter().map(|l| l.name.clone()).collect()
}

/// Style for bold labels / title in the detail pane.
fn bold() -> Style {
    Style {
        color: ColorStyle::terminal_default(),
        effects: EnumSet::only(Effect::Bold).into(),
    }
}

/// Append a bold field label (e.g. "URL:    ") followed by a plain value line.
fn append_field(s: &mut StyledString, label: &str, value: &str) {
    s.append_styled(label, bold());
    s.append_plain(value);
    s.append_plain("\n");
}

fn body_excerpt(pr: &PullRequest) -> String {
    pr.body
        .as_deref()
        .unwrap_or("")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join("\n")
}

fn detail_header(pr: &PullRequest) -> StyledString {
    let labels = pr_label_names(pr);
    let labels_str = if labels.is_empty() {
        "none".to_string()
    } else {
        labels.join(", ")
    };

    let mut s = StyledString::new();

    // Bold title line
    s.append_styled(format!("PR #{}: ", pr.number), bold());
    s.append_styled(&pr.title, bold());
    s.append_plain("\n\n");

    // Fields: bold label, plain value
    append_field(&mut s, "URL:    ", &pr.html_url);

    s.append_styled("State:  ", bold());
    s.append_plain(&pr.state);
    s.append_plain("  |  ");
    s.append_styled("Author: ", bold());
    s.append_plain(format!("@{}\n", pr.user.login));

    append_field(&mut s, "Labels: ", &labels_str);
    append_field(&mut s, "Base:   ", &pr.base.ref_name);

    s
}

/// Data fetched in the background for the full detail view.
struct PrDetails {
    statuses: HashMap<String, CiStatus>,
    comments: Vec<IssueComment>,
    files: Vec<PrFile>,
}

// ── Immediate summary (shown before background fetch completes) ───────────────

fn pr_summary(pr: &PullRequest) -> StyledString {
    let mut s = detail_header(pr);
    append_stats_line(&mut s, pr);
    s.append_plain("\n");
    s.append_styled("CI Status:", bold());
    s.append_plain(" [Loading…]");

    let excerpt = body_excerpt(pr);
    if !excerpt.is_empty() {
        s.append_plain("\n\n");
        s.append_styled("Description:", bold());
        s.append_plain(format!("\n{}", excerpt));
    }
    s
}

/// Append the compact stats line (Commits / Comments / Reviews) using whatever
/// fields the PR list response populated.
fn append_stats_line(s: &mut StyledString, pr: &PullRequest) {
    let mut has = false;
    let mut sep = |s: &mut StyledString| {
        if has {
            s.append_plain("  |  ");
        }
        has = true;
    };
    if let Some(n) = pr.commits {
        sep(&mut *s);
        s.append_styled("Commits: ", bold());
        s.append_plain(n.to_string());
    }
    if let Some(n) = pr.comments {
        sep(&mut *s);
        s.append_styled("Comments: ", bold());
        s.append_plain(n.to_string());
    }
    if let Some(n) = pr.review_comments {
        sep(&mut *s);
        s.append_styled("Reviews: ", bold());
        s.append_plain(n.to_string());
    }
    if has {
        s.append_plain("\n");
    }
}

// ── Full summary (replaces immediate version when background fetch finishes) ──

fn pr_summary_full(pr: &PullRequest, details: PrDetails) -> StyledString {
    let PrDetails {
        statuses,
        comments,
        files,
    } = details;

    let mut s = detail_header(pr);
    append_stats_line(&mut s, pr);

    // ── CI status ────────────────────────────────────────────────────────────
    s.append_plain("\n");
    s.append_styled("CI Status:\n", bold());
    let mut ci_entries: Vec<(String, CiStatus)> = statuses.into_iter().collect();
    ci_entries.sort_by_key(|(k, _)| k.clone());
    if ci_entries.is_empty() {
        s.append_plain("  (no CI status reported)\n");
    } else {
        for (ctx_name, job) in &ci_entries {
            let icon = match job.state.as_str() {
                "success" => "✓",
                "failure" | "error" => "✗",
                "pending" => "⏳",
                _ => "?",
            };
            let has_url = job.url.is_some() && job.state != "pending";
            s.append_plain(format!("  {} {}", icon, ctx_name));
            if has_url {
                s.append_plain("  ");
                s.append_styled("[i:Inspect]", Style::from(Color::Dark(BaseColor::Cyan)));
            }
            s.append_plain("\n");
        }
    }

    // ── Changed files ────────────────────────────────────────────────────────
    if !files.is_empty() {
        let fetched_adds: u64 = files.iter().map(|f| f.additions).sum();
        let fetched_dels: u64 = files.iter().map(|f| f.deletions).sum();
        // Use PR-level totals if available (they cover all files, not just the
        // first 100 from the files endpoint).
        let total_adds = pr.additions.unwrap_or(fetched_adds);
        let total_dels = pr.deletions.unwrap_or(fetched_dels);
        let file_count = pr.changed_files.unwrap_or(files.len() as u64);

        s.append_plain("\n");
        s.append_styled(
            format!(
                "Changed files ({} files, +{} / -{}):\n",
                file_count, total_adds, total_dels
            ),
            bold(),
        );
        const MAX_FILES: usize = 15;
        for file in files.iter().take(MAX_FILES) {
            let icon = match file.status.as_str() {
                "added" => "A",
                "removed" | "deleted" => "D",
                "modified" | "changed" => "M",
                "renamed" => "R",
                "copied" => "C",
                _ => "?",
            };
            let name = if file.status == "renamed" {
                if let Some(ref prev) = file.previous_filename {
                    format!("{} → {}", truncate(prev, 24), truncate(&file.filename, 24))
                } else {
                    truncate(&file.filename, 50).to_string()
                }
            } else {
                truncate(&file.filename, 50).to_string()
            };
            s.append_plain(format!(
                "  {}  {}  (+{} / -{})\n",
                icon, name, file.additions, file.deletions
            ));
        }
        if files.len() > MAX_FILES {
            s.append_plain(format!("  … and {} more\n", files.len() - MAX_FILES));
        }
    }

    // ── Description ──────────────────────────────────────────────────────────
    if let Some(ref body) = pr.body {
        if !body.trim().is_empty() {
            s.append_plain("\n");
            s.append_styled("Description:\n", bold());
            s.append(crate::md_render::render(body));
        }
    }

    // ── Comments ─────────────────────────────────────────────────────────────
    if !comments.is_empty() {
        s.append_plain("\n");
        s.append_styled(format!("Comments ({}):\n", comments.len()), bold());
        let sep = "\u{2500}".repeat(40);
        for (i, c) in comments.iter().enumerate() {
            let date = c.created_at.get(..10).unwrap_or(&c.created_at);
            if i > 0 {
                s.append_plain(format!("{}\n", sep));
            }
            s.append_styled(format!("@{} [{}]:\n", c.user.login, date), bold());
            s.append(crate::md_render::render_indented(&c.body, "  "));
            s.append_plain("\n");
        }
    }

    s
}

fn update_detail_from_pr(siv: &mut Cursive, pr: &PullRequest) {
    let content = pr_summary(pr);
    siv.call_on_name("pr_detail", |v: &mut TextView| {
        v.set_content(content);
    });
}

fn update_detail_full(siv: &mut Cursive, pr: &PullRequest, details: PrDetails) {
    let content = pr_summary_full(pr, details);
    siv.call_on_name("pr_detail", |v: &mut TextView| {
        v.set_content(content);
    });
}

fn fetch_pr_details_in_background(siv: &mut Cursive, gh: Arc<PrClient>, pr: PullRequest) {
    let (offline, profile) = siv
        .user_data::<TuiState>()
        .map(|t| (t.offline, t.profile.clone()))
        .unwrap_or((false, String::new()));
    let db = siv.user_data::<TuiState>().and_then(|t| t.db.clone());

    if offline {
        // Serve from cache — no network call.
        if let Some(ref db) = db {
            if let Ok(Some(cached)) = db.load_pr_details(&profile, pr.number) {
                // Reassemble CiStatus from statuses + status_urls.
                let statuses: HashMap<String, CiStatus> = cached
                    .statuses
                    .iter()
                    .map(|(ctx, state)| {
                        let url = cached.status_urls.get(ctx).cloned();
                        (
                            ctx.clone(),
                            CiStatus {
                                state: state.clone(),
                                url,
                            },
                        )
                    })
                    .collect();
                let ci_statuses = statuses.clone();
                if let Some(t) = siv.user_data::<TuiState>() {
                    t.ci_statuses = ci_statuses;
                }
                let details = PrDetails {
                    statuses,
                    comments: cached.comments,
                    files: cached.files,
                };
                update_detail_full(siv, &pr, details);
            }
        }
        return;
    }

    let pr_number = pr.number;
    let sha = pr.head.sha.clone();
    let cb = siv.cb_sink().clone();
    std::thread::spawn(move || {
        let mut critical_fetch_failed = false;
        // All fetches run sequentially in the background thread.
        // most_recent_statuses() returns HashMap<String, CiStatus>.
        let statuses = gh.most_recent_statuses(&sha).unwrap_or_else(|e| {
            eprintln!(
                "Warning: failed to fetch CI statuses for PR {}: {}",
                pr_number, e
            );
            Default::default()
        });
        let comments = gh.get_all_issue_comments(pr_number).unwrap_or_else(|e| {
            eprintln!(
                "Warning: failed to fetch comments for PR {}: {}",
                pr_number, e
            );
            Default::default()
        });
        let files = gh.get_pr_files(pr_number).unwrap_or_else(|e| {
            eprintln!("Warning: failed to fetch files for PR {}: {}", pr_number, e);
            critical_fetch_failed = true;
            Default::default()
        });

        // Split CiStatus map into (state strings, url strings) for cache storage.
        let cached_states: HashMap<String, String> = statuses
            .iter()
            .map(|(ctx, job)| (ctx.clone(), job.state.clone()))
            .collect();
        let cached_urls: HashMap<String, String> = statuses
            .iter()
            .filter_map(|(ctx, job)| job.url.as_ref().map(|u| (ctx.clone(), u.clone())))
            .collect();

        // Persist so the next offline session can show these details.
        // Skip cache write if any critical fetch failed to avoid storing incomplete data.
        if let Some(ref db) = db {
            if !critical_fetch_failed {
                let cached = crate::db::CachedPrDetails {
                    statuses: cached_states,
                    comments: comments.clone(),
                    files: files.clone(),
                    commits: vec![],
                    status_urls: cached_urls,
                };
                let updated_at = pr.updated_at.as_deref();
                if let Err(e) = db.cache_pr_details(&profile, pr_number, &cached, updated_at) {
                    eprintln!("Warning: failed to cache PR details: {}", e);
                }
            }
        }

        let ci_statuses = statuses.clone();
        let details = PrDetails {
            statuses,
            comments,
            files,
        };
        cb.send(Box::new(move |s: &mut Cursive| {
            let current = selected_pr(s).map(|p| p.number);
            if current == Some(pr_number) {
                // Store statuses in TuiState so the 'i' key handler can open the job selector.
                if let Some(t) = s.user_data::<TuiState>() {
                    t.ci_statuses = ci_statuses;
                }
                update_detail_full(s, &pr, details);
            }
        }))
        .ok();
    });
}

// ─── Review view ─────────────────────────────────────────────────────────────

#[derive(Clone)]
enum ReviewItemKind {
    FileHeader,
    HunkHeader,
    Added,
    Removed,
    Context,
    ReviewComment,
    IssueComment,
}

#[derive(Clone)]
struct ReviewItem {
    kind: ReviewItemKind,
    /// File path this item belongs to (None for general comments).
    path: Option<String>,
    /// Line number in the new (RIGHT) file, when applicable.
    new_line: Option<u64>,
    content: String,
}

struct ReviewData {
    files: Vec<PrFile>,
    review_comments: Vec<ReviewComment>,
    issue_comments: Vec<IssueComment>,
}

/// Parse `@@ -old_start[,count] +new_start[,count] @@` and return (old, new) start lines.
fn parse_hunk_header(line: &str) -> Option<(u64, u64)> {
    let inner = line.trim_start_matches('@').trim_start_matches(' ');
    let mut parts = inner.split_whitespace();
    let old_part = parts.next()?.trim_start_matches('-');
    let new_part = parts.next()?.trim_start_matches('+');
    let old_start: u64 = old_part.split(',').next()?.parse().ok()?;
    let new_start: u64 = new_part.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

/// Format a line number for the dual-column gutter.  Returns 5 right-aligned
/// digits, or 5 spaces when the number is not applicable for that side.
fn fmt_lineno(n: Option<u64>) -> String {
    match n {
        Some(n) => format!("{:>5}", n),
        None => "     ".to_string(),
    }
}

/// Hard-wrap `text` into chunks of at most `max_width` Unicode scalar values.
/// An empty input yields one empty string so blank comment lines are preserved.
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut remaining = text;
    loop {
        if remaining.is_empty() {
            break;
        }
        let split = remaining
            .char_indices()
            .nth(max_width)
            .map(|(i, _)| i)
            .unwrap_or(remaining.len());
        lines.push(remaining[..split].to_string());
        remaining = &remaining[split..];
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Expand a review/issue comment into one header item plus one item per
/// (wrapped) body line so the full text is visible in the SelectView.
/// `body_width` is the number of characters available for body text after
/// the 4-space indent prefix.
fn make_review_comment_items(
    user: &str,
    date: &str,
    body: &str,
    path: Option<String>,
    new_line: Option<u64>,
    kind: ReviewItemKind,
    body_width: usize,
) -> Vec<ReviewItem> {
    let mut result = Vec::new();
    result.push(ReviewItem {
        kind: kind.clone(),
        path: path.clone(),
        new_line,
        content: format!("  ▶ @{} [{}]:", user, date),
    });
    for raw_line in body.lines() {
        for wrapped in wrap_text(raw_line, body_width) {
            result.push(ReviewItem {
                kind: kind.clone(),
                path: path.clone(),
                new_line,
                content: format!("    {}", wrapped),
            });
        }
    }
    result
}

/// Build a flat list of review items from diff + comment data.
///
/// Line gutter format (13 chars before the diff content):
///   `{old:>5}  {new:>5} {raw}`
/// where `old`/`new` are blank when not applicable for that side, and `raw`
/// already starts with `+`, `-`, or ` `.
///
/// `body_width` is passed to comment wrapping (terminal width minus borders
/// and the 4-char comment indent).
fn build_review_items(data: &ReviewData, body_width: usize) -> Vec<ReviewItem> {
    let mut items: Vec<ReviewItem> = Vec::new();

    for file in &data.files {
        items.push(ReviewItem {
            kind: ReviewItemKind::FileHeader,
            path: Some(file.filename.clone()),
            new_line: None,
            content: format!(
                "━━ {} (+{} / -{}) ━━",
                file.filename, file.additions, file.deletions
            ),
        });

        if let Some(patch) = &file.patch {
            let mut new_line: u64 = 0;
            let mut old_line: u64 = 0;

            for raw in patch.lines() {
                if raw.starts_with("@@") {
                    if let Some((old_s, new_s)) = parse_hunk_header(raw) {
                        old_line = old_s;
                        new_line = new_s;
                    }
                    // Indent hunk header to match the gutter width (5+2+5+1 = 13).
                    items.push(ReviewItem {
                        kind: ReviewItemKind::HunkHeader,
                        path: Some(file.filename.clone()),
                        new_line: None,
                        content: format!("             {}", raw),
                    });
                } else if raw.starts_with('+') && !raw.starts_with("+++") {
                    let this_new = new_line;
                    items.push(ReviewItem {
                        kind: ReviewItemKind::Added,
                        path: Some(file.filename.clone()),
                        new_line: Some(this_new),
                        content: format!(
                            "{}  {} {}",
                            fmt_lineno(None),
                            fmt_lineno(Some(this_new)),
                            raw
                        ),
                    });
                    // Inline RIGHT-side review comments for this line.
                    for c in data.review_comments.iter().filter(|c| {
                        c.path == file.filename
                            && c.line == Some(this_new)
                            && c.side.as_deref() != Some("LEFT")
                    }) {
                        let date = &c.created_at[..10.min(c.created_at.len())];
                        items.extend(make_review_comment_items(
                            &c.user.login,
                            date,
                            &c.body,
                            Some(file.filename.clone()),
                            Some(this_new),
                            ReviewItemKind::ReviewComment,
                            body_width,
                        ));
                    }
                    new_line += 1;
                } else if raw.starts_with('-') && !raw.starts_with("---") {
                    let this_old = old_line;
                    items.push(ReviewItem {
                        kind: ReviewItemKind::Removed,
                        path: Some(file.filename.clone()),
                        new_line: None,
                        content: format!(
                            "{}  {} {}",
                            fmt_lineno(Some(this_old)),
                            fmt_lineno(None),
                            raw
                        ),
                    });
                    // Inline LEFT-side review comments.
                    for c in data.review_comments.iter().filter(|c| {
                        c.path == file.filename
                            && c.original_line == Some(this_old)
                            && c.side.as_deref() == Some("LEFT")
                    }) {
                        let date = &c.created_at[..10.min(c.created_at.len())];
                        items.extend(make_review_comment_items(
                            &c.user.login,
                            date,
                            &c.body,
                            Some(file.filename.clone()),
                            None,
                            ReviewItemKind::ReviewComment,
                            body_width,
                        ));
                    }
                    old_line += 1;
                } else if raw.starts_with(' ') {
                    let this_new = new_line;
                    items.push(ReviewItem {
                        kind: ReviewItemKind::Context,
                        path: Some(file.filename.clone()),
                        new_line: Some(this_new),
                        content: format!(
                            "{}  {} {}",
                            fmt_lineno(Some(old_line)),
                            fmt_lineno(Some(this_new)),
                            raw
                        ),
                    });
                    new_line += 1;
                    old_line += 1;
                }
            }
        } else {
            items.push(ReviewItem {
                kind: ReviewItemKind::Context,
                path: Some(file.filename.clone()),
                new_line: None,
                content: "  (binary or very large file — no patch available)".to_string(),
            });
        }

        // File-level review comments (no specific line).
        for c in data
            .review_comments
            .iter()
            .filter(|c| c.path == file.filename && c.line.is_none())
        {
            let date = &c.created_at[..10.min(c.created_at.len())];
            items.extend(make_review_comment_items(
                &c.user.login,
                date,
                &c.body,
                Some(file.filename.clone()),
                None,
                ReviewItemKind::ReviewComment,
                body_width,
            ));
        }
    }

    // General PR comments (issue comments).
    if !data.issue_comments.is_empty() {
        items.push(ReviewItem {
            kind: ReviewItemKind::FileHeader,
            path: None,
            new_line: None,
            content: "━━ General comments ━━".to_string(),
        });
        for c in &data.issue_comments {
            let date = &c.created_at[..10.min(c.created_at.len())];
            items.extend(make_review_comment_items(
                &c.user.login,
                date,
                &c.body,
                None,
                None,
                ReviewItemKind::IssueComment,
                body_width,
            ));
        }
    }

    items
}

fn review_item_label(item: &ReviewItem) -> StyledString {
    match item.kind {
        ReviewItemKind::FileHeader => StyledString::styled(&item.content, bold()),
        ReviewItemKind::HunkHeader => StyledString::styled(
            &item.content,
            Style::from(ColorStyle::front(Color::Dark(BaseColor::Cyan))),
        ),
        ReviewItemKind::Added => StyledString::styled(
            &item.content,
            Style::from(ColorStyle::front(Color::Dark(BaseColor::Green))),
        ),
        ReviewItemKind::Removed => StyledString::styled(
            &item.content,
            Style::from(ColorStyle::front(Color::Dark(BaseColor::Red))),
        ),
        ReviewItemKind::Context => {
            StyledString::styled(&item.content, Style::from(ColorStyle::terminal_default()))
        }
        ReviewItemKind::ReviewComment | ReviewItemKind::IssueComment => StyledString::styled(
            &item.content,
            Style::from(ColorStyle::front(Color::Dark(BaseColor::Yellow))),
        ),
    }
}

fn show_review_view(siv: &mut Cursive, gh: Arc<PrClient>, pr: PullRequest) {
    let offline = siv
        .user_data::<TuiState>()
        .map(|t| t.offline)
        .unwrap_or(false);
    if offline {
        show_error(
            siv,
            "Review diff not available in offline mode (diff data is not cached).",
        );
        return;
    }
    siv.add_layer(loading_dialog("Fetching PR diff and comments…"));
    let cb = siv.cb_sink().clone();
    let gh2 = Arc::clone(&gh);
    let pr2 = pr.clone();
    std::thread::spawn(move || {
        let pr_number = pr2.number;
        let files = match gh2.get_pr_files(pr_number) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Warning: failed to fetch files for PR {}: {}", pr_number, e);
                let msg = format!("Failed to fetch PR diff: {}", e);
                cb.send(Box::new(move |s: &mut Cursive| {
                    s.pop_layer(); // remove loading dialog
                    show_error(s, &msg);
                }))
                .ok();
                return;
            }
        };
        let review_comments = gh2.list_review_comments(pr_number).unwrap_or_else(|e| {
            eprintln!(
                "Warning: failed to fetch review comments for PR {}: {}",
                pr_number, e
            );
            Default::default()
        });
        let issue_comments = gh2
            .get_last_issue_comments(pr_number, 100)
            .unwrap_or_else(|e| {
                eprintln!(
                    "Warning: failed to fetch issue comments for PR {}: {}",
                    pr_number, e
                );
                Default::default()
            });
        let data = ReviewData {
            files,
            review_comments,
            issue_comments,
        };
        let gh3 = gh2;
        cb.send(Box::new(move |s: &mut Cursive| {
            s.pop_layer();
            build_review_layer(s, gh3, pr2, data);
        }))
        .ok();
    });
}

fn build_review_layer(siv: &mut Cursive, gh: Arc<PrClient>, pr: PullRequest, data: ReviewData) {
    // Panel border = 2 chars; comment indent prefix "    " = 4 chars.
    let body_width = siv.screen_size().x.saturating_sub(6);
    let items = build_review_items(&data, body_width);

    // Store session so we can rebuild the layer after the comment form closes.
    if let Some(state) = siv.user_data::<TuiState>() {
        state.review = Some(ReviewSession {
            pr: pr.clone(),
            items: items.clone(),
            commit_id: pr.head.sha.clone(),
        });
    }

    assemble_review_layer(siv, gh, 0);
}

/// Build (or rebuild) the fullscreen review layer, optionally pre-scrolled to
/// `focus_idx` (the SelectView row to show near the top).
fn assemble_review_layer(siv: &mut Cursive, gh: Arc<PrClient>, focus_idx: usize) {
    let session = match siv.user_data::<TuiState>().and_then(|s| s.review.clone()) {
        Some(s) => s,
        None => return,
    };

    let tui_keys = siv
        .user_data::<TuiState>()
        .map(|s| s.tui_keys.clone())
        .unwrap_or_default();
    let key_comment = tui_keys.review_comment;
    let key_down = tui_keys.review_down;
    let key_up = tui_keys.review_up;
    let key_close = tui_keys.review_close;

    let pr = session.pr.clone();
    let items = session.items.clone();
    let pr_number = pr.number;
    let commit_id = session.commit_id.clone();

    let mut select = SelectView::<ReviewItem>::new();
    for item in &items {
        let label = review_item_label(item);
        select.add_item(label, item.clone());
    }
    if focus_idx > 0 {
        let _ = select.set_selection(focus_idx);
    }

    let gh_c = Arc::clone(&gh);
    let commit_c = commit_id.clone();
    let gh_enter = Arc::clone(&gh);
    let commit_enter = commit_id.clone();

    let review_scroll = ScrollView::new(select.with_name("review_list")).with_name("review_scroll");

    let view = OnEventView::new(review_scroll)
        .on_event(key_comment, move |s| {
            let result = s
                .call_on_name("review_list", |v: &mut SelectView<ReviewItem>| {
                    let idx = v.selected_id().unwrap_or(0);
                    let sel = v.selection().map(|rc| (*rc).clone());
                    (idx, sel)
                })
                .unwrap_or((0, None));
            s.pop_layer();
            open_review_comment_form(
                s,
                Arc::clone(&gh_c),
                pr_number,
                commit_c.clone(),
                result.0,
                result.1,
            );
        })
        .on_event(cursive::event::Key::Enter, move |s| {
            let result = s
                .call_on_name("review_list", |v: &mut SelectView<ReviewItem>| {
                    let idx = v.selected_id().unwrap_or(0);
                    let sel = v.selection().map(|rc| (*rc).clone());
                    (idx, sel)
                })
                .unwrap_or((0, None));
            s.pop_layer();
            open_review_comment_form(
                s,
                Arc::clone(&gh_enter),
                pr_number,
                commit_enter.clone(),
                result.0,
                result.1,
            );
        })
        .on_event(key_down, |s| {
            if let Some(cb) = s.call_on_name("review_list", |v: &mut SelectView<ReviewItem>| {
                v.select_down(1)
            }) {
                cb(s);
            }
        })
        .on_event(key_up, |s| {
            if let Some(cb) = s.call_on_name("review_list", |v: &mut SelectView<ReviewItem>| {
                v.select_up(1)
            }) {
                cb(s);
            }
        })
        .on_event(key_close, |s| {
            s.pop_layer();
        })
        .on_event(cursive::event::Key::Esc, |s| {
            s.pop_layer();
        });

    let help = TextView::new(format!(
        " {}/Enter:Comment on line  {}/↓:Down  {}/↑:Up  {}/Esc:Close",
        tui_keys.review_comment, tui_keys.review_down, tui_keys.review_up, tui_keys.review_close,
    ));

    let layout = LinearLayout::vertical()
        .child(help)
        .child(view.full_screen());

    let panel = Panel::new(layout).title(format!(
        "Review PR #{}: {}",
        pr.number,
        truncate(&pr.title, 60)
    ));

    siv.add_fullscreen_layer(panel);

    // After the first render, scroll so focus_idx − 5 is at the top.
    if focus_idx > 0 {
        let scroll_to = focus_idx.saturating_sub(5);
        let cb = siv.cb_sink().clone();
        cb.send(Box::new(move |s: &mut Cursive| {
            s.call_on_name(
                "review_scroll",
                |v: &mut ScrollView<NamedView<SelectView<ReviewItem>>>| {
                    let _ = v.set_offset((0usize, scroll_to));
                },
            );
        }))
        .ok();
    }
}

/// Rebuild the review layer (called after the comment form is dismissed).
fn restore_review_layer(siv: &mut Cursive, focus_idx: usize) {
    let gh = match siv
        .user_data::<TuiState>()
        .and_then(|s| s.pr_client.clone())
    {
        Some(g) => g,
        None => return,
    };
    assemble_review_layer(siv, gh, focus_idx);
}

/// Replace the review layer with a split view: diff on top (scrolled to
/// context), comment form on the bottom.
fn open_review_comment_form(
    siv: &mut Cursive,
    gh: Arc<PrClient>,
    pr_number: u64,
    commit_id: String,
    focus_idx: usize,
    selected: Option<ReviewItem>,
) {
    // Build styled text for the read-only diff display at the top.
    // Show ALL content so the user can scroll up for earlier context.
    let content: StyledString = siv
        .user_data::<TuiState>()
        .and_then(|s| s.review.clone())
        .map(|sess| {
            let mut out = StyledString::new();
            for item in &sess.items {
                out.append(review_item_label(item));
                out.append_plain("\n");
            }
            out
        })
        .unwrap_or_default();

    let screen_height = siv.screen_size().y;
    // Form: ~3 labels + 2 EditViews + TextArea(6) + buttons + panel borders ≈ 14 rows.
    let form_height: usize = 14;
    let review_height = screen_height.saturating_sub(form_height + 2).max(5);

    let default_path = selected
        .as_ref()
        .and_then(|i| i.path.clone())
        .unwrap_or_default();
    let default_line = selected
        .as_ref()
        .and_then(|i| i.new_line)
        .map(|n| n.to_string())
        .unwrap_or_default();

    let gh_post = Arc::clone(&gh);
    let commit_post = commit_id.clone();
    let focus_post = focus_idx;
    let focus_cancel = focus_idx;

    let split_layer = LinearLayout::vertical()
        .child(
            Panel::new(ScrollView::new(TextView::new(content)).with_name("split_review_scroll"))
                .title("Diff")
                .fixed_height(review_height),
        )
        .child(
            Dialog::around(
                LinearLayout::vertical()
                    .child(TextView::new("File path:"))
                    .child(
                        EditView::new()
                            .content(&default_path)
                            .with_name("rc_path")
                            .min_width(72),
                    )
                    .child(TextView::new("Line number (RIGHT side):"))
                    .child(
                        EditView::new()
                            .content(&default_line)
                            .with_name("rc_line")
                            .min_width(10),
                    )
                    .child(TextView::new("Comment (Tab=newline, Ctrl-h=backspace):"))
                    .child(TextArea::new().with_name("rc_body").min_size((72, 6))),
            )
            .title("Add Line Comment")
            .button("Post", move |s| {
                let path = s
                    .call_on_name("rc_path", |v: &mut EditView| v.get_content().to_string())
                    .unwrap_or_default();
                let line_str = s
                    .call_on_name("rc_line", |v: &mut EditView| v.get_content().to_string())
                    .unwrap_or_default();
                let body = s
                    .call_on_name("rc_body", |v: &mut TextArea| v.get_content().to_string())
                    .unwrap_or_default();
                let trimmed_line = line_str.trim().to_string();
                let line: u64 = match trimmed_line.parse() {
                    Ok(n) if n > 0 => n,
                    Ok(_) => {
                        show_error(s, "Line number must be a positive integer.");
                        return;
                    }
                    Err(_) if trimmed_line.is_empty() => {
                        show_error(s, "Line number is required.");
                        return;
                    }
                    Err(_) => {
                        show_error(s, "Line number must be a positive integer.");
                        return;
                    }
                };
                if path.trim().is_empty() || body.trim().is_empty() {
                    show_error(s, "File path and comment text are required.");
                    return;
                }
                let Some((offline, provider)) = tui_offline_and_provider(s) else {
                    show_error(s, "Internal error: TUI state missing.");
                    return;
                };
                let db = s.user_data::<TuiState>().and_then(|t| t.db.clone());
                if offline {
                    s.pop_layer();
                    restore_review_layer(s, focus_post);
                    if provider == crate::db::Provider::Forgejo {
                        show_error(s, FORGEJO_OFFLINE_MSG);
                    } else if let Some(db) = db {
                        match db.queue_action(&crate::db::ProviderAction {
                            provider,
                            action: crate::db::QueuedAction::PostReviewComment {
                                pr_number,
                                commit_id: commit_post.clone(),
                                path,
                                line,
                                body,
                            },
                        }) {
                            Ok(()) => show_info(s, "Review comment queued for sync."),
                            Err(e) => show_error(s, &format!("Failed to queue action: {}", e)),
                        }
                    } else {
                        show_error(s, "Offline mode but no local database configured.");
                    }
                } else {
                    s.pop_layer();
                    restore_review_layer(s, focus_post);
                    let gh3 = Arc::clone(&gh_post);
                    let commit3 = commit_post.clone();
                    let is_forgejo = provider == crate::db::Provider::Forgejo;
                    do_in_background(
                        s,
                        "Posting comment…",
                        move || gh3.create_review_comment(pr_number, &commit3, &path, line, &body),
                        move |s, res| match res {
                            Ok(()) => {
                                if is_forgejo {
                                    show_info(s, "Comment posted as issue comment (Forgejo inline review comments not yet supported).");
                                } else {
                                    show_info(s, "Comment posted successfully.");
                                }
                            }
                            Err(e) => show_error(s, &format!("Failed to post comment: {}", e)),
                        },
                    );
                }
            })
            .button("Cancel", move |s| {
                s.pop_layer();
                restore_review_layer(s, focus_cancel);
            })
            .full_width(),
        );

    let split_layer = OnEventView::new(split_layer).on_event(cursive::event::Key::Esc, move |s| {
        s.pop_layer();
        restore_review_layer(s, focus_cancel);
    });

    siv.add_fullscreen_layer(split_layer);

    // set_offset clamps against last_available_size(), which is (0,0) until
    // the first layout pass runs.  Queue a first callback that itself queues
    // a second one; by the time the second fires, one layout pass has already
    // completed so last_available_size() is correct and the offset sticks.
    let scroll_to = focus_idx.saturating_sub(5);
    let cb = siv.cb_sink().clone();
    cb.send(Box::new(move |s: &mut Cursive| {
        let cb2 = s.cb_sink().clone();
        cb2.send(Box::new(move |s: &mut Cursive| {
            s.call_on_name("split_review_scroll", |v: &mut ScrollView<TextView>| {
                v.set_offset((0usize, scroll_to));
            });
        }))
        .ok();
    }))
    .ok();
}

// ─── Action dialog (Enter key) ────────────────────────────────────────────────

fn show_action_dialog(siv: &mut Cursive, gh: Arc<PrClient>, pr: PullRequest) {
    let tui_keys = siv
        .user_data::<TuiState>()
        .map(|s| s.tui_keys.clone())
        .unwrap_or_default();
    let key_review = tui_keys.action_review;
    let key_labels = tui_keys.action_labels;
    let key_push = tui_keys.action_push;
    let key_backport = tui_keys.action_backport;

    let pr_number = pr.number;
    let pr_title = truncate(&pr.title, 50);
    let pr_url = pr.html_url.clone();
    let labels = pr_label_names(&pr);
    let is_acked = labels.iter().any(|l| l == "ack");
    let is_rejected = labels.iter().any(|l| l == "rejected");
    let is_pushed = labels.iter().any(|l| l == "pushed");
    let is_closed = pr.state == "closed";

    let mut dlg = Dialog::new().title(format!("PR #{}: {}", pr_number, pr_title));

    // Review is always first so it gets default focus when the dialog opens.
    let gh_v = Arc::clone(&gh);
    let pr_v = pr.clone();
    dlg = dlg.button(format!("Review ({})", key_review), move |s| {
        s.pop_layer();
        show_review_view(s, Arc::clone(&gh_v), pr_v.clone());
    });

    let gh_l = Arc::clone(&gh);
    let pr_l = pr.clone();
    dlg = dlg.button(format!("Labels ({})", key_labels), move |s| {
        s.pop_layer();
        show_label_editor(s, Arc::clone(&gh_l), pr_l.clone());
    });

    if !is_closed && !is_rejected && !is_acked {
        let gh_a = Arc::clone(&gh);
        dlg = dlg.button(format!("ACK ({})", tui_keys.ack), move |s| {
            s.pop_layer();
            show_ack_dialog(s, Arc::clone(&gh_a), pr_number);
        });
    }
    if !is_closed && !is_rejected {
        let gh_x = Arc::clone(&gh);
        dlg = dlg.button(format!("Reject ({})", tui_keys.reject), move |s| {
            s.pop_layer();
            show_reject_dialog(s, Arc::clone(&gh_x), pr_number);
        });
    }
    if !is_closed && is_acked && !is_rejected && !is_pushed {
        dlg = dlg.button(format!("Push ({})", key_push), move |s| {
            s.pop_layer();
            show_push_dialog(s, pr_number);
        });
    }
    if is_acked && !is_rejected {
        dlg = dlg.button(format!("Backport ({})", key_backport), move |s| {
            s.pop_layer();
            show_backport_dialog(s, pr_number);
        });
    }

    let dlg = dlg
        .button(format!("Browser ({})", tui_keys.browser), move |_s| {
            open_browser(&pr_url)
        })
        .button("Close (Esc)", |s| {
            s.pop_layer();
        });

    let gh_v2 = Arc::clone(&gh);
    let pr_v2 = pr.clone();
    let gh_l2 = Arc::clone(&gh);
    let pr_l2 = pr.clone();
    let dlg = OnEventView::new(dlg)
        .on_event(cursive::event::Key::Esc, |s| {
            s.pop_layer();
        })
        .on_event(key_review, move |s| {
            s.pop_layer();
            show_review_view(s, Arc::clone(&gh_v2), pr_v2.clone());
        })
        .on_event(key_labels, move |s| {
            s.pop_layer();
            show_label_editor(s, Arc::clone(&gh_l2), pr_l2.clone());
        })
        .on_event(key_push, move |s| {
            if !is_closed && is_acked && !is_rejected && !is_pushed {
                s.pop_layer();
                show_push_dialog(s, pr_number);
            }
        })
        .on_event(key_backport, move |s| {
            if is_acked && !is_rejected {
                s.pop_layer();
                show_backport_dialog(s, pr_number);
            }
        });

    siv.add_layer(dlg);
}

// ─── Label editor ────────────────────────────────────────────────────────────

fn show_label_editor(siv: &mut Cursive, gh: Arc<PrClient>, pr: PullRequest) {
    siv.add_layer(loading_dialog("Fetching labels…"));
    let cb = siv.cb_sink().clone();
    let gh2 = Arc::clone(&gh);
    std::thread::spawn(move || {
        let result = gh2.list_repo_labels();
        cb.send(Box::new(move |s: &mut Cursive| {
            s.pop_layer();
            match result {
                Err(e) => show_error(s, &format!("Failed to fetch labels: {}", e)),
                Ok(repo_labels) => build_label_editor_layer(s, gh2, pr, repo_labels),
            }
        }))
        .ok();
    });
}

fn build_label_editor_layer(
    siv: &mut Cursive,
    gh: Arc<PrClient>,
    pr: PullRequest,
    repo_labels: Vec<Label>,
) {
    let key_quit = siv
        .user_data::<TuiState>()
        .map(|s| s.tui_keys.quit)
        .unwrap_or('q');

    let left_width = compute_left_width(siv.screen_size().x);
    let pr_current: Vec<String> = pr.labels.iter().map(|l| l.name.clone()).collect();
    let pr_number = pr.number;

    // ── Left pane: PR info + current labels ───────────────────────────────────
    let mut left_text = StyledString::new();
    left_text.append_styled(format!("PR #{}\n", pr.number), bold());
    left_text.append_styled("Title:  ", bold());
    left_text.append_plain(format!("{}\n", pr.title));
    left_text.append_styled("Author: ", bold());
    left_text.append_plain(format!("@{}\n", pr.user.login));
    left_text.append_styled("State:  ", bold());
    left_text.append_plain(format!("{}\n\n", pr.state));
    left_text.append_styled("Current labels:\n", bold());
    if pr_current.is_empty() {
        left_text.append_plain("  (none)\n");
    } else {
        for name in &pr_current {
            left_text.append_plain(format!("  • {}\n", name));
        }
    }

    let left_panel = Panel::new(ScrollView::new(TextView::new(left_text)))
        .title(format!("PR #{}", pr.number))
        .full_width();

    // ── Right pane: one checkbox per repo label ────────────────────────────────
    let mut cb_layout = LinearLayout::vertical();
    for (i, label) in repo_labels.iter().enumerate() {
        let checked = pr_current.contains(&label.name);
        let mut checkbox = Checkbox::new();
        checkbox.set_checked(checked);
        cb_layout = cb_layout.child(
            LinearLayout::horizontal()
                .child(checkbox.with_name(format!("label_cb_{}", i)))
                .child(TextView::new(format!("  {}", label.name))),
        );
    }

    let gh_apply = Arc::clone(&gh);
    let labels_apply = repo_labels.clone();
    let current_apply = pr_current.clone();

    let right_pane = Dialog::around(ScrollView::new(cb_layout).full_screen())
        .title("Labels  (Space=toggle  Tab=next)")
        .button("Apply", move |s| {
            let mut to_add: Vec<String> = Vec::new();
            let mut to_remove: Vec<String> = Vec::new();
            for (i, label) in labels_apply.iter().enumerate() {
                let is_checked = s
                    .call_on_name(&format!("label_cb_{}", i), |v: &mut Checkbox| {
                        v.is_checked()
                    })
                    .unwrap_or(false);
                let was_checked = current_apply.contains(&label.name);
                if is_checked && !was_checked {
                    to_add.push(label.name.clone());
                } else if !is_checked && was_checked {
                    to_remove.push(label.name.clone());
                }
            }
            if to_add.is_empty() && to_remove.is_empty() {
                s.pop_layer();
                return;
            }
            let Some((offline, provider)) = tui_offline_and_provider(s) else {
                show_error(s, "Internal error: TUI state missing.");
                return;
            };
            let db = s.user_data::<TuiState>().and_then(|t| t.db.clone());
            s.pop_layer();
            if offline {
                if provider == crate::db::Provider::Forgejo {
                    show_error(s, FORGEJO_OFFLINE_MSG);
                } else if let Some(db) = db {
                    match db.queue_action(&crate::db::ProviderAction {
                        provider,
                        action: crate::db::QueuedAction::UpdateLabels {
                            pr_number,
                            to_add,
                            to_remove,
                        },
                    }) {
                        Ok(()) => show_info(s, "Label changes queued for sync."),
                        Err(e) => show_error(s, &format!("Failed to queue action: {}", e)),
                    }
                } else {
                    show_error(s, "Offline mode but no local database configured.");
                }
            } else {
                s.add_layer(loading_dialog("Updating labels…"));
                let cb = s.cb_sink().clone();
                let gh2 = Arc::clone(&gh_apply);
                std::thread::spawn(move || {
                    let result: Result<PullRequest> = (|| {
                        if !to_add.is_empty() {
                            let refs: Vec<&str> = to_add.iter().map(|s| s.as_str()).collect();
                            gh2.add_labels(pr_number, &refs)?;
                        }
                        for label in &to_remove {
                            gh2.remove_label(pr_number, label)?;
                        }
                        // Re-fetch so the UI reflects the new label set.
                        gh2.get_pr(pr_number)
                    })();
                    cb.send(Box::new(move |s: &mut Cursive| {
                        s.pop_layer(); // remove loading dialog
                        match result {
                            Err(e) => show_error(s, &format!("Failed to update labels: {}", e)),
                            Ok(updated_pr) => {
                                update_detail_from_pr(s, &updated_pr);
                                show_info(s, "Labels updated successfully.");
                            }
                        }
                    }))
                    .ok();
                });
            }
        })
        .button("Cancel", |s| {
            s.pop_layer();
        })
        .fixed_width(left_width);

    let help = TextView::new(format!(" Space:Toggle  Tab:Next  {}/Esc:Cancel", key_quit));

    let layout = LinearLayout::vertical().child(help).child(
        LinearLayout::horizontal()
            .child(right_pane)
            .child(left_panel)
            .full_screen(),
    );

    let layout = OnEventView::new(layout)
        .on_event(cursive::event::Key::Esc, |s| {
            s.pop_layer();
        })
        .on_event(key_quit, |s| {
            s.pop_layer();
        });

    siv.add_fullscreen_layer(layout);
}

// ─── ACK dialog ──────────────────────────────────────────────────────────────

fn show_ack_dialog(siv: &mut Cursive, gh: Arc<PrClient>, pr_number: u64) {
    let gh2 = Arc::clone(&gh);
    siv.add_layer(
        Dialog::new()
            .title(format!("ACK PR #{}", pr_number))
            .content(
                LinearLayout::vertical()
                    .child(TextView::new("Optional comment:"))
                    .child(EditView::new().with_name("ack_comment").min_width(60)),
            )
            .button("ACK", move |s| {
                let comment = s
                    .call_on_name("ack_comment", |v: &mut EditView| {
                        v.get_content().to_string()
                    })
                    .unwrap_or_default();
                let Some((offline, provider)) = tui_offline_and_provider(s) else {
                    show_error(s, "Internal error: TUI state missing.");
                    return;
                };
                let db = s.user_data::<TuiState>().and_then(|t| t.db.clone());
                s.pop_layer();
                if offline {
                    if provider == crate::db::Provider::Forgejo {
                        show_error(s, FORGEJO_OFFLINE_MSG);
                    } else if let Some(db) = db {
                        match db.queue_action(&crate::db::ProviderAction {
                            provider,
                            action: crate::db::QueuedAction::Ack {
                                pr_number,
                                comment: Some(comment).filter(|c| !c.is_empty()),
                            },
                        }) {
                            Ok(()) => {
                                show_info(s, &format!("ACK for PR #{} queued for sync.", pr_number))
                            }
                            Err(e) => show_error(s, &format!("Failed to queue action: {}", e)),
                        }
                    } else {
                        show_error(s, "Offline mode but no local database configured.");
                    }
                } else {
                    do_in_background(
                        s,
                        "ACKing PR…",
                        {
                            let gh3 = Arc::clone(&gh2);
                            move || {
                                super::pr_ack::run_api(
                                    &gh3,
                                    pr_number,
                                    Some(comment.as_str()).filter(|s| !s.is_empty()),
                                )
                            }
                        },
                        move |s, res| match res {
                            Ok(()) => {
                                show_info(s, &format!("PR #{} ACKed successfully.", pr_number))
                            }
                            Err(e) => show_error(s, &format!("ACK failed: {}", e)),
                        },
                    );
                }
            })
            .button("Cancel", |s| {
                s.pop_layer();
            }),
    );
}

// ─── Reject dialog ───────────────────────────────────────────────────────────

fn show_reject_dialog(siv: &mut Cursive, gh: Arc<PrClient>, pr_number: u64) {
    let gh2 = Arc::clone(&gh);
    siv.add_layer(
        Dialog::new()
            .title(format!("Reject PR #{}", pr_number))
            .content(
                LinearLayout::vertical()
                    .child(TextView::new("Reason (required):"))
                    .child(EditView::new().with_name("reject_reason").min_width(60)),
            )
            .button("Reject", move |s| {
                let reason = s
                    .call_on_name("reject_reason", |v: &mut EditView| {
                        v.get_content().to_string()
                    })
                    .unwrap_or_default();
                if reason.trim().is_empty() {
                    show_error(s, "A reason is required.");
                    return;
                }
                let Some((offline, provider)) = tui_offline_and_provider(s) else {
                    show_error(s, "Internal error: TUI state missing.");
                    return;
                };
                let db = s.user_data::<TuiState>().and_then(|t| t.db.clone());
                s.pop_layer();
                if offline {
                    if provider == crate::db::Provider::Forgejo {
                        show_error(s, FORGEJO_OFFLINE_MSG);
                    } else if let Some(db) = db {
                        match db.queue_action(&crate::db::ProviderAction {
                            provider,
                            action: crate::db::QueuedAction::Reject {
                                pr_number,
                                comment: reason,
                            },
                        }) {
                            Ok(()) => show_info(
                                s,
                                &format!("Reject for PR #{} queued for sync.", pr_number),
                            ),
                            Err(e) => show_error(s, &format!("Failed to queue action: {}", e)),
                        }
                    } else {
                        show_error(s, "Offline mode but no local database configured.");
                    }
                } else {
                    do_in_background(
                        s,
                        "Rejecting PR…",
                        {
                            let gh3 = Arc::clone(&gh2);
                            move || super::pr_reject::run_api(&gh3, pr_number, &reason)
                        },
                        move |s, res| match res {
                            Ok(()) => show_info(s, &format!("PR #{} rejected.", pr_number)),
                            Err(e) => show_error(s, &format!("Reject failed: {}", e)),
                        },
                    );
                }
            })
            .button("Cancel", |s| {
                s.pop_layer();
            }),
    );
}

// ─── Push dialog ─────────────────────────────────────────────────────────────

fn show_push_dialog(siv: &mut Cursive, pr_number: u64) {
    siv.add_layer(
        Dialog::new()
            .title(format!("Push PR #{}", pr_number))
            .content(
                LinearLayout::vertical()
                    .child(TextView::new(
                        "Reviewer(s) — comma-separated login or \"Name <email>\":",
                    ))
                    .child(EditView::new().with_name("push_reviewer").min_width(64))
                    .child(TextView::new(""))
                    .child(TextView::new(
                        "Backport to branches — comma-separated (e.g. ipa-4-12,ipa-4-11):",
                    ))
                    .child(EditView::new().with_name("push_backport").min_width(64))
                    .child(TextView::new(""))
                    .child(
                        LinearLayout::horizontal()
                            .child(Checkbox::new().with_name("push_autobackport"))
                            .child(TextView::new("  Auto-backport from PR labels")),
                    ),
            )
            .button("Execute", move |s| {
                let reviewer_str = s
                    .call_on_name("push_reviewer", |v: &mut EditView| {
                        v.get_content().to_string()
                    })
                    .unwrap_or_default();
                let backport_str = s
                    .call_on_name("push_backport", |v: &mut EditView| {
                        v.get_content().to_string()
                    })
                    .unwrap_or_default();
                let autobackport = s
                    .call_on_name("push_autobackport", |v: &mut Checkbox| v.is_checked())
                    .unwrap_or(false);

                let reviewers: Vec<String> = reviewer_str
                    .split(',')
                    .map(|r| r.trim().to_string())
                    .filter(|r| !r.is_empty())
                    .collect();
                let backport_branches: Vec<String> = backport_str
                    .split(',')
                    .map(|b| b.trim().to_string())
                    .filter(|b| !b.is_empty())
                    .collect();

                if let Some(state) = s.user_data::<TuiState>() {
                    state.pending_action = Some(PendingTuiAction::PrPush {
                        pr_id: pr_number,
                        reviewers,
                        backport_branches,
                        autobackport,
                    });
                    s.quit();
                } else {
                    show_error(s, "Internal error: TUI state missing. Push aborted.");
                }
            })
            .button("Cancel", |s| {
                s.pop_layer();
            }),
    );
}

// ─── Backport dialog ──────────────────────────────────────────────────────────

fn show_backport_dialog(siv: &mut Cursive, pr_number: u64) {
    siv.add_layer(
        Dialog::new()
            .title(format!("Backport PR #{}", pr_number))
            .content(
                LinearLayout::vertical()
                    .child(TextView::new(
                        "Target branches — comma-separated (e.g. ipa-4-12,ipa-4-11):",
                    ))
                    .child(EditView::new().with_name("bp_branches").min_width(64)),
            )
            .button("Execute", move |s| {
                let branches_str = s
                    .call_on_name("bp_branches", |v: &mut EditView| {
                        v.get_content().to_string()
                    })
                    .unwrap_or_default();
                let branches: Vec<String> = branches_str
                    .split(',')
                    .map(|b| b.trim().to_string())
                    .filter(|b| !b.is_empty())
                    .collect();

                if branches.is_empty() {
                    show_error(s, "At least one target branch is required.");
                    return;
                }

                if let Some(state) = s.user_data::<TuiState>() {
                    state.pending_action = Some(PendingTuiAction::Backport {
                        pr_id: pr_number,
                        branches,
                    });
                    s.quit();
                } else {
                    show_error(s, "Internal error: TUI state missing. Backport aborted.");
                }
            })
            .button("Cancel", |s| {
                s.pop_layer();
            }),
    );
}

// ─── Offline sync ─────────────────────────────────────────────────────────────

fn sync_queued_actions(siv: &mut Cursive) {
    let gh = match siv
        .user_data::<TuiState>()
        .and_then(|t| t.pr_client.clone())
    {
        Some(g) => g,
        None => {
            show_error(siv, "No PR client configured.");
            return;
        }
    };
    let db = match siv.user_data::<TuiState>().and_then(|t| t.db.clone()) {
        Some(d) => d,
        None => {
            show_info(siv, "No local database – nothing to sync.");
            return;
        }
    };
    let actions = match db.pending_actions() {
        Ok(a) => a,
        Err(e) => {
            show_error(siv, &format!("Failed to read queue: {}", e));
            return;
        }
    };
    if actions.is_empty() {
        show_info(siv, "No queued actions to sync.");
        return;
    }

    siv.add_layer(loading_dialog(&format!(
        "Syncing {} queued action(s)…",
        actions.len()
    )));
    let cb = siv.cb_sink().clone();
    std::thread::spawn(move || {
        let mut ok = 0usize;
        let mut first_error: Option<String> = None;
        for pa in actions {
            let result = match pa.provider_action.provider {
                crate::db::Provider::GitHub => apply_queued_action(&gh, &pa.provider_action.action),
                crate::db::Provider::Forgejo => {
                    // PrClient already routes to the correct forge backend internally;
                    // no Forgejo-specific dispatch is needed here.
                    apply_queued_action(&gh, &pa.provider_action.action)
                }
                crate::db::Provider::Pagure => Err(anyhow::anyhow!(
                    "Pagure provider is not yet supported for offline sync."
                )),
            };
            if let Err(e) = result {
                first_error = Some(format!("{:?}: {}", pa.provider_action.action, e));
                break; // stop on first error to preserve ordering
            } else if let Err(e) = db.delete_action(pa.id) {
                eprintln!(
                    "Warning: action applied but failed to delete from queue (id={}): {}. \
                     Manual cleanup of the offline queue may be required.",
                    pa.id, e
                );
                break;
            } else {
                ok += 1;
            }
        }
        cb.send(Box::new(move |s: &mut Cursive| {
            s.pop_layer();
            if first_error.is_none() {
                show_info(s, &format!("Synced {} action(s) successfully.", ok));
            } else {
                show_error(
                    s,
                    &format!(
                        "Synced {} action(s). First error:\n{}",
                        ok,
                        first_error.as_deref().unwrap_or("unknown error")
                    ),
                );
            }
            // Refresh PR list after sync
            let gh2 = s.user_data::<TuiState>().and_then(|t| t.pr_client.clone());
            if let Some(gh2) = gh2 {
                refresh_list(s, gh2);
            }
        }))
        .ok();
    });
}

fn apply_queued_action(gh: &Arc<PrClient>, action: &crate::db::QueuedAction) -> anyhow::Result<()> {
    use crate::db::QueuedAction::*;
    match action {
        AddLabel { pr_number, label } => gh.add_labels(*pr_number, &[label.as_str()]),
        RemoveLabel { pr_number, label } => gh.remove_label(*pr_number, label),
        PostComment { pr_number, body } => gh.create_comment(*pr_number, body),
        PostReviewComment {
            pr_number,
            commit_id,
            path,
            line,
            body,
        } => gh.create_review_comment(*pr_number, commit_id, path, *line, body),
        Ack { pr_number, comment } => super::pr_ack::run_api(gh, *pr_number, comment.as_deref()),
        Reject { pr_number, comment } => super::pr_reject::run_api(gh, *pr_number, comment),
        UpdateLabels {
            pr_number,
            to_add,
            to_remove,
        } => {
            if !to_add.is_empty() {
                let refs: Vec<&str> = to_add.iter().map(|s| s.as_str()).collect();
                gh.add_labels(*pr_number, &refs)?;
            }
            for label in to_remove {
                gh.remove_label(*pr_number, label)?;
            }
            Ok(())
        }
    }
}

// ─── Background action helper ─────────────────────────────────────────────────

fn do_in_background<A, D>(siv: &mut Cursive, msg: &str, action: A, on_done: D)
where
    A: FnOnce() -> Result<()> + Send + 'static,
    D: FnOnce(&mut Cursive, Result<()>) + Send + 'static,
{
    siv.add_layer(loading_dialog(msg));
    let cb = siv.cb_sink().clone();
    std::thread::spawn(move || {
        let result = action();
        cb.send(Box::new(move |s: &mut Cursive| {
            s.pop_layer();
            on_done(s, result);
        }))
        .ok();
    });
}

// ─── PR row styling ───────────────────────────────────────────────────────────

/// Build a colour-coded row for the PR list.
/// `inner_width` is the usable width inside the panel border.
fn pr_row_styled(pr: &PullRequest, inner_width: usize) -> StyledString {
    let label_names: Vec<&str> = pr.labels.iter().map(|l| l.name.as_str()).collect();

    // Use Dark base-color variants: they have sufficient contrast on both
    // light-background and dark-background terminals, unlike Light variants
    // which disappear on white/light backgrounds.
    // Pending PRs use the terminal's own foreground (always readable) — they
    // carry no special status so they don't need a colour signal.
    let style: Style = if label_names.contains(&"rejected") || pr.state == "closed" {
        Style {
            color: ColorStyle::front(Color::Dark(BaseColor::Red)),
            effects: EnumSet::only(Effect::Bold).into(),
        }
    } else if label_names.contains(&"ack") {
        Style {
            color: ColorStyle::front(Color::Dark(BaseColor::Green)),
            effects: EnumSet::only(Effect::Bold).into(),
        }
    } else {
        Style::from(ColorStyle::terminal_default())
    };

    let state_icon = if pr.is_merged() {
        "✓"
    } else if pr.state == "open" {
        "○"
    } else {
        "✗"
    };

    // Build label suffix first so we know its length.
    let label_str = if label_names.is_empty() {
        String::new()
    } else {
        let shown: Vec<&str> = label_names.iter().copied().take(2).collect();
        let suffix = if label_names.len() > 2 { "…" } else { "" };
        format!(" [{}{}]", shown.join(","), suffix)
    };

    // "#NNNNNN ○ " is 10 chars; leave room for label_str at the end.
    let prefix_len = 10usize;
    let title_max = inner_width
        .saturating_sub(prefix_len + label_str.len())
        .max(8);

    let text = format!(
        "#{:<6} {} {}{}",
        pr.number,
        state_icon,
        truncate(&pr.title, title_max),
        label_str,
    );

    StyledString::styled(text, style)
}

// ─── Small helpers ────────────────────────────────────────────────────────────

/// Returns `Some((offline, provider))` from the current TuiState.
/// Returns `None` when TuiState is absent; callers should handle this as an
/// internal error and show an error dialog rather than silently using defaults.
fn tui_offline_and_provider(s: &mut Cursive) -> Option<(bool, crate::db::Provider)> {
    let state = s.user_data::<TuiState>()?;
    let provider = state.pr_client.as_ref()?.provider();
    Some((state.offline, provider))
}

fn show_info(siv: &mut Cursive, msg: &str) {
    siv.add_layer(Dialog::text(msg).title("Done").button("OK", |s| {
        s.pop_layer();
    }));
}

fn show_error(siv: &mut Cursive, msg: &str) {
    siv.add_layer(Dialog::text(msg).title("Error").button("OK", |s| {
        s.pop_layer();
    }));
}

fn loading_dialog(msg: &str) -> impl cursive::View {
    Dialog::text(msg)
}

fn open_browser(url: &str) {
    if let Err(e) = std::process::Command::new("xdg-open").arg(url).spawn() {
        eprintln!("Warning: could not open browser ({}): {}", url, e);
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let mut collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        collected.pop(); // remove last char to make room for ellipsis
        format!("{}…", collected)
    } else {
        collected
    }
}

// ─── CI job results viewer ────────────────────────────────────────────────────

/// Show a dialog listing completed CI jobs with result URLs for the selected PR.
/// If there is exactly one such job, jump directly into the file browser.
fn show_ci_job_selector(siv: &mut Cursive, pr: PullRequest) {
    let ci_statuses = siv
        .user_data::<TuiState>()
        .map(|t| t.ci_statuses.clone())
        .unwrap_or_default();

    // Collect completed jobs (not pending) that have an artifact URL.
    let mut jobs: Vec<(String, String, String)> = ci_statuses
        .iter()
        .filter(|(_, job)| job.url.is_some() && job.state != "pending")
        .map(|(ctx, job)| (ctx.clone(), job.url.clone().unwrap(), job.state.clone()))
        .collect();
    jobs.sort_by(|a, b| a.0.cmp(&b.0));

    if jobs.is_empty() {
        show_info(
            siv,
            &format!("PR #{}: no completed CI jobs with result URLs.", pr.number),
        );
        return;
    }

    if jobs.len() == 1 {
        let (name, url, _) = jobs.remove(0);
        show_job_results_view(siv, name, url);
        return;
    }

    let mut select = SelectView::<(String, String)>::new();
    for (name, url, state) in &jobs {
        let icon = match state.as_str() {
            "success" => "✓",
            "failure" | "error" => "✗",
            _ => "?",
        };
        select.add_item(format!("{} {}", icon, name), (name.clone(), url.clone()));
    }

    select.set_on_submit(|s, item: &(String, String)| {
        s.pop_layer();
        show_job_results_view(s, item.0.clone(), item.1.clone());
    });

    let dlg = Dialog::around(select)
        .title(format!("CI Jobs — PR #{}", pr.number))
        .button("Cancel", |s| {
            s.pop_layer();
        });

    let dlg = OnEventView::new(dlg).on_event(cursive::event::Key::Esc, |s| {
        s.pop_layer();
    });

    siv.add_layer(dlg);
}

/// Open a full-screen two-pane file browser for a CI job's artifacts.
///
/// Left pane: artifact listing (`SelectView<ArtifactEntry>` named `"ci_files"`).
/// Right pane: rendered content (`ScrollView<TextView>` named `"ci_content"`).
/// Backspace navigates to the parent directory; `q`/`Esc` closes the view.
///
/// Fetched artifact content is cached in memory for the lifetime of the view
/// so that revisiting a file does not re-download it.
fn show_job_results_view(siv: &mut Cursive, job_name: String, base_url: String) {
    let viewer_box = match crate::ci::get_viewer(&base_url) {
        Some(v) => v,
        None => {
            show_info(
                siv,
                &format!("No CI viewer available for URL:\n{}", base_url),
            );
            return;
        }
    };
    let viewer: Arc<dyn crate::ci::CiJobViewer> = Arc::from(viewer_box);

    // URL navigation stack: the last entry is the currently displayed directory.
    let url_stack: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![base_url.clone()]));

    // In-session file content cache: URL → rendered StyledString.
    // Avoids re-downloading a file the user already viewed.
    let content_cache: Arc<Mutex<HashMap<String, StyledString>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // In-session directory listing cache: URL → artifact entries.
    // Once a job's report URL is live, its directory contents never change.
    let listing_cache: Arc<Mutex<HashMap<String, Vec<crate::ci::ArtifactEntry>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // In-session test sub-entry index: report URL → test ArtifactEntry list.
    // Populated when a pytest report is parsed; lets populate_ci_files re-attach
    // test entries to the file list after directory navigation without re-fetching.
    let sub_entries_cache: Arc<Mutex<HashMap<String, Vec<crate::ci::ArtifactEntry>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Tracks which pane (left/right) currently has focus for the Tab handler.
    // false = left pane (ci_files SelectView); true = right pane (ci_content_scroll).
    let focus_right: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

    // Plain-text mirror of whatever is currently shown in the right pane.
    // Updated whenever ci_content is set so the 'b' handler can read it without
    // re-fetching anything.
    let current_content: Arc<Mutex<StyledString>> = Arc::new(Mutex::new(StyledString::plain("")));

    // ── Artifact list (left pane) ─────────────────────────────────────────────
    let viewer_select = Arc::clone(&viewer);
    let viewer_submit = Arc::clone(&viewer);
    let url_stack_submit = Arc::clone(&url_stack);
    let cache_select = Arc::clone(&content_cache);
    let cache_submit = Arc::clone(&content_cache);
    let cache_back = Arc::clone(&content_cache);
    let cache_init = Arc::clone(&content_cache);
    let listing_submit = Arc::clone(&listing_cache);
    let listing_back = Arc::clone(&listing_cache);
    let listing_init = Arc::clone(&listing_cache);
    let sub_select = Arc::clone(&sub_entries_cache);
    let sub_submit = Arc::clone(&sub_entries_cache);
    let sub_back = Arc::clone(&sub_entries_cache);
    let sub_init = Arc::clone(&sub_entries_cache);
    let content_select = Arc::clone(&current_content);
    let content_submit = Arc::clone(&current_content);
    let content_back = Arc::clone(&current_content);
    let content_init = Arc::clone(&current_content);
    let content_bug = Arc::clone(&current_content);

    let mut file_select = SelectView::<crate::ci::ArtifactEntry>::new();

    // on_select: serve from cache when available; otherwise fetch in background.
    file_select.set_on_select(move |s, entry: &crate::ci::ArtifactEntry| {
        // Separator entries (visual dividers between files and tests) are inert.
        if entry.url.starts_with("sep://") {
            return;
        }
        if entry.is_dir {
            s.call_on_name("ci_content", |v: &mut TextView| {
                v.set_content("(directory — press Enter to navigate in)");
            });
            return;
        }

        // Cache hit: no network request needed.
        if let Some(cached) = cache_select.lock().unwrap().get(&entry.url).cloned() {
            *content_select.lock().unwrap() = cached.clone();
            s.call_on_name("ci_content", |v: &mut TextView| {
                v.set_content(cached);
            });
            s.call_on_name(
                "ci_content_scroll",
                |v: &mut ScrollView<NamedView<TextView>>| {
                    v.set_offset(cursive::Vec2::new(0, 0));
                },
            );
            return;
        }

        let entry = entry.clone();
        let viewer2 = Arc::clone(&viewer_select);
        let cache2 = Arc::clone(&cache_select);
        let sub2 = Arc::clone(&sub_select);
        let content_ref = Arc::clone(&content_select);
        let cb = s.cb_sink().clone();
        std::thread::spawn(move || {
            match viewer2.fetch_content(&entry) {
                Ok(crate::ci::ContentResult {
                    content,
                    sub_entries,
                }) => {
                    cache2
                        .lock()
                        .unwrap()
                        .insert(entry.url.clone(), content.clone());
                    // Pre-load per-test log content into the content cache and
                    // record the test artifact entries for later re-attachment.
                    let sub_artifacts: Vec<crate::ci::ArtifactEntry> = sub_entries
                        .into_iter()
                        .map(|(artifact, log)| {
                            cache2.lock().unwrap().insert(artifact.url.clone(), log);
                            artifact
                        })
                        .collect();
                    if !sub_artifacts.is_empty() {
                        sub2.lock()
                            .unwrap()
                            .insert(entry.url.clone(), sub_artifacts.clone());
                    }
                    cb.send(Box::new(move |s: &mut Cursive| {
                        *content_ref.lock().unwrap() = content.clone();
                        s.call_on_name("ci_content", |v: &mut TextView| {
                            v.set_content(content);
                        });
                        s.call_on_name(
                            "ci_content_scroll",
                            |v: &mut ScrollView<NamedView<TextView>>| {
                                v.set_offset(cursive::Vec2::new(0, 0));
                            },
                        );
                        if !sub_artifacts.is_empty() {
                            append_sub_entries_to_list(s, sub_artifacts);
                        }
                    }))
                    .ok();
                }
                Err(e) => {
                    cb.send(Box::new(move |s: &mut Cursive| {
                        s.call_on_name("ci_content", |v: &mut TextView| {
                            v.set_content(format!("Error loading content:\n{}", e));
                        });
                    }))
                    .ok();
                }
            }
        });
    });

    // on_submit (Enter): navigate into a directory.
    file_select.set_on_submit(move |s, entry: &crate::ci::ArtifactEntry| {
        if !entry.is_dir {
            return; // file content already shown by on_select
        }
        let entry_url = entry.url.clone();
        {
            url_stack_submit.lock().unwrap().push(entry_url.clone());
        }

        // Listing cache hit: no network request needed.
        if let Some(cached) = listing_submit.lock().unwrap().get(&entry_url).cloned() {
            let viewer2 = Arc::clone(&viewer_submit);
            let cache2 = Arc::clone(&cache_submit);
            let sub2 = Arc::clone(&sub_submit);
            let cc2 = Arc::clone(&content_submit);
            populate_ci_files(s, cached, viewer2, cache2, sub2, cc2);
            return;
        }

        let viewer2 = Arc::clone(&viewer_submit);
        let url_stack2 = Arc::clone(&url_stack_submit);
        let listing2 = Arc::clone(&listing_submit);
        let cache2 = Arc::clone(&cache_submit);
        let sub2 = Arc::clone(&sub_submit);
        let cc2 = Arc::clone(&content_submit);
        let cb = s.cb_sink().clone();
        std::thread::spawn(move || {
            match viewer2.list_artifacts(&entry_url) {
                Ok(entries) => {
                    listing2
                        .lock()
                        .unwrap()
                        .insert(entry_url.clone(), entries.clone());
                    let viewer3 = Arc::clone(&viewer2);
                    cb.send(Box::new(move |s: &mut Cursive| {
                        populate_ci_files(s, entries, viewer3, cache2, sub2, cc2);
                    }))
                    .ok();
                }
                Err(e) => {
                    // Undo the push since navigation failed.
                    url_stack2.lock().unwrap().pop();
                    cb.send(Box::new(move |s: &mut Cursive| {
                        show_error(s, &format!("Failed to load directory:\n{}", e));
                    }))
                    .ok();
                }
            }
        });
    });

    // ── Layout ────────────────────────────────────────────────────────────────
    let job_name_short = truncate(&job_name, 55);
    let ci_left_width = (siv.screen_size().x * 2 / 5).clamp(48, 72);

    // No external ScrollView: SelectView handles its own scrolling via
    // make_important_visible, which only works correctly when SelectView is
    // given a constrained height (not the unlimited height a ScrollView offers).
    let left_panel = Panel::new(BoxedView::boxed(
        file_select.with_name("ci_files").full_height(),
    ))
    .title(panel_title("Artifacts", true))
    .with_name("ci_left_panel")
    .fixed_width(ci_left_width);

    let right_panel = Panel::new(BoxedView::boxed(
        ScrollView::new(TextView::new("Loading…").with_name("ci_content"))
            .with_name("ci_content_scroll")
            .full_screen(),
    ))
    .title(panel_title(&job_name_short, false))
    .with_name("ci_right_panel")
    .full_width();

    let help = TextView::new(
        " ↓/↑:Navigate  PgUp/PgDn:Page  Home/End:Jump  Tab:Switch Pane  Enter:Open  Backspace:Parent  b:Bug  q/Esc:Close",
    );

    let body = LinearLayout::horizontal()
        .child(left_panel)
        .child(right_panel)
        .full_screen();

    let layout = LinearLayout::vertical().child(help).child(body);

    // ── Key handling (outer wrapper) ──────────────────────────────────────────
    let viewer_back = Arc::clone(&viewer);
    let url_stack_back = Arc::clone(&url_stack);
    // Extra clones so the close handlers can clear both caches immediately on
    // exit, rather than waiting for background threads to drop their Arc clones.
    let cache_q = Arc::clone(&content_cache);
    let listing_q = Arc::clone(&listing_cache);
    let sub_q = Arc::clone(&sub_entries_cache);
    let cache_esc = Arc::clone(&content_cache);
    let listing_esc = Arc::clone(&listing_cache);
    let sub_esc = Arc::clone(&sub_entries_cache);
    // Arcs shared into the on_pre_event_inner closures.
    // Each closure is FnMut; it clones the Arc/String on every keypress into
    // the EventResult::with_cb callback that cursive schedules.
    let fr_tab = Arc::clone(&focus_right);
    let fr_up = Arc::clone(&focus_right);
    let fr_down = Arc::clone(&focus_right);
    let fr_pgup = Arc::clone(&focus_right);
    let fr_pgdn = Arc::clone(&focus_right);
    let fr_home = Arc::clone(&focus_right);
    let fr_end = Arc::clone(&focus_right);
    let job_name_tab = job_name_short.clone();
    let job_name_bug = job_name_short;

    let layout = OnEventView::new(layout)
        .on_event('q', move |s| {
            cache_q.lock().unwrap().clear();
            listing_q.lock().unwrap().clear();
            sub_q.lock().unwrap().clear();
            s.pop_layer();
        })
        .on_event(cursive::event::Key::Esc, move |s| {
            cache_esc.lock().unwrap().clear();
            listing_esc.lock().unwrap().clear();
            sub_esc.lock().unwrap().clear();
            s.pop_layer();
        })
        .on_event('b', move |s| {
            show_file_bug_dialog(s, &job_name_bug, &content_bug);
        })
        // Tab: intercepted before LinearLayout moves cursive focus between children.
        // Uses simulated focus (Arc flag) so behaviour is independent of whether
        // the inner views accept real cursive focus.
        .on_pre_event_inner(cursive::event::Key::Tab, move |_, _| {
            let fr = Arc::clone(&fr_tab);
            let jn = job_name_tab.clone();
            Some(EventResult::with_cb(move |s: &mut Cursive| {
                let is_right = {
                    let mut f = fr.lock().unwrap();
                    *f = !*f;
                    *f
                };
                s.call_on_name("ci_left_panel", |p: &mut Panel<BoxedView>| {
                    p.set_title(panel_title("Artifacts", !is_right));
                });
                s.call_on_name("ci_right_panel", |p: &mut Panel<BoxedView>| {
                    p.set_title(panel_title(&jn, is_right));
                });
            }))
        })
        // Up / Down: when right pane is "focused" intercept and scroll it;
        // otherwise return None so SelectView receives the event through the
        // normal cursive dispatch path and its make_important_visible keeps the
        // highlighted item in the visible portion of the left pane.
        .on_pre_event_inner(cursive::event::Key::Up, move |_, _| {
            let fr = Arc::clone(&fr_up);
            if *fr.lock().unwrap() {
                Some(EventResult::with_cb(|s: &mut Cursive| {
                    scroll_ci_content(s, -1);
                }))
            } else {
                None
            }
        })
        .on_pre_event_inner(cursive::event::Key::Down, move |_, _| {
            let fr = Arc::clone(&fr_down);
            if *fr.lock().unwrap() {
                Some(EventResult::with_cb(|s: &mut Cursive| {
                    scroll_ci_content(s, 1);
                }))
            } else {
                None
            }
        })
        // PageUp / PageDown: 10-line jumps.
        .on_pre_event_inner(cursive::event::Key::PageUp, move |_, _| {
            let fr = Arc::clone(&fr_pgup);
            Some(EventResult::with_cb(move |s: &mut Cursive| {
                if *fr.lock().unwrap() {
                    scroll_ci_content(s, -10);
                } else if let Some(cb) = s.call_on_name(
                    "ci_files",
                    |v: &mut SelectView<crate::ci::ArtifactEntry>| v.select_up(10),
                ) {
                    cb(s);
                }
            }))
        })
        .on_pre_event_inner(cursive::event::Key::PageDown, move |_, _| {
            let fr = Arc::clone(&fr_pgdn);
            Some(EventResult::with_cb(move |s: &mut Cursive| {
                if *fr.lock().unwrap() {
                    scroll_ci_content(s, 10);
                } else if let Some(cb) = s.call_on_name(
                    "ci_files",
                    |v: &mut SelectView<crate::ci::ArtifactEntry>| v.select_down(10),
                ) {
                    cb(s);
                }
            }))
        })
        // Home / End: jump to first / last item (left pane) or top / bottom (right).
        .on_pre_event_inner(cursive::event::Key::Home, move |_, _| {
            let fr = Arc::clone(&fr_home);
            Some(EventResult::with_cb(move |s: &mut Cursive| {
                if *fr.lock().unwrap() {
                    s.call_on_name(
                        "ci_content_scroll",
                        |v: &mut ScrollView<NamedView<TextView>>| {
                            v.set_offset(cursive::Vec2::new(0, 0));
                        },
                    );
                } else if let Some(cb) = s.call_on_name(
                    "ci_files",
                    // 10_000 saturates at item 0 without integer overflow.
                    |v: &mut SelectView<crate::ci::ArtifactEntry>| v.select_up(10_000),
                ) {
                    cb(s);
                }
            }))
        })
        .on_pre_event_inner(cursive::event::Key::End, move |_, _| {
            let fr = Arc::clone(&fr_end);
            Some(EventResult::with_cb(move |s: &mut Cursive| {
                if *fr.lock().unwrap() {
                    s.call_on_name(
                        "ci_content_scroll",
                        |v: &mut ScrollView<NamedView<TextView>>| {
                            // Offset is clamped to valid range during the next layout pass.
                            v.set_offset(cursive::Vec2::new(0, usize::MAX / 2));
                        },
                    );
                } else if let Some(cb) = s.call_on_name(
                    "ci_files",
                    // 10_000 saturates at the last item without integer overflow.
                    |v: &mut SelectView<crate::ci::ArtifactEntry>| v.select_down(10_000),
                ) {
                    cb(s);
                }
            }))
        })
        .on_event(cursive::event::Key::Backspace, move |s| {
            // Pop the current directory and show the parent listing.
            let parent_url = {
                let mut stack = url_stack_back.lock().unwrap();
                if stack.len() <= 1 {
                    return;
                }
                stack.pop();
                stack.last().cloned()
            };
            if let Some(url) = parent_url {
                // Listing cache hit: instant navigation, no network.
                if let Some(cached) = listing_back.lock().unwrap().get(&url).cloned() {
                    let viewer2 = Arc::clone(&viewer_back);
                    let cache2 = Arc::clone(&cache_back);
                    let sub2 = Arc::clone(&sub_back);
                    let cc2 = Arc::clone(&content_back);
                    populate_ci_files(s, cached, viewer2, cache2, sub2, cc2);
                    return;
                }

                let viewer2 = Arc::clone(&viewer_back);
                let url_stack2 = Arc::clone(&url_stack_back);
                let listing2 = Arc::clone(&listing_back);
                let cache2 = Arc::clone(&cache_back);
                let sub2 = Arc::clone(&sub_back);
                let cc2 = Arc::clone(&content_back);
                let cb = s.cb_sink().clone();
                std::thread::spawn(move || {
                    match viewer2.list_artifacts(&url) {
                        Ok(entries) => {
                            listing2
                                .lock()
                                .unwrap()
                                .insert(url.clone(), entries.clone());
                            let viewer3 = Arc::clone(&viewer2);
                            cb.send(Box::new(move |s: &mut Cursive| {
                                populate_ci_files(s, entries, viewer3, cache2, sub2, cc2);
                            }))
                            .ok();
                        }
                        Err(e) => {
                            // Restore the URL we popped since re-fetch failed.
                            url_stack2.lock().unwrap().push(url);
                            cb.send(Box::new(move |s: &mut Cursive| {
                                show_error(s, &format!("Failed to load parent listing:\n{}", e));
                            }))
                            .ok();
                        }
                    }
                });
            }
        });

    siv.add_fullscreen_layer(layout);

    // ── Background: fetch initial artifact listing ────────────────────────────
    let viewer_init = Arc::clone(&viewer);
    let cb = siv.cb_sink().clone();
    std::thread::spawn(move || match viewer_init.list_artifacts(&base_url) {
        Ok(entries) => {
            listing_init
                .lock()
                .unwrap()
                .insert(base_url.clone(), entries.clone());
            let viewer2 = Arc::clone(&viewer_init);
            cb.send(Box::new(move |s: &mut Cursive| {
                populate_ci_files(s, entries, viewer2, cache_init, sub_init, content_init);
            }))
            .ok();
        }
        Err(e) => {
            cb.send(Box::new(move |s: &mut Cursive| {
                s.call_on_name("ci_content", |v: &mut TextView| {
                    v.set_content(format!("Error loading artifact listing:\n{}", e));
                });
            }))
            .ok();
        }
    });
}

/// Scroll the CI content pane by `lines` (positive = down, negative = up).
fn scroll_ci_content(siv: &mut Cursive, lines: i32) {
    siv.call_on_name(
        "ci_content_scroll",
        |v: &mut ScrollView<NamedView<TextView>>| {
            let cur = v.get_scroller().content_viewport().top() as i32;
            let next = (cur + lines).max(0) as usize;
            v.set_offset(cursive::Vec2::new(0, next));
        },
    );
}

/// Populate `"ci_files"` SelectView with `entries`, auto-select the default
/// entry (e.g. `report.html`), and immediately show its content (from cache
/// if available, otherwise fetch in background and store in cache).
/// If the default entry has associated test sub-entries in `sub_entries_cache`
/// they are appended to the file list below a visual separator.
fn populate_ci_files(
    siv: &mut Cursive,
    entries: Vec<crate::ci::ArtifactEntry>,
    viewer: Arc<dyn crate::ci::CiJobViewer>,
    cache: Arc<Mutex<HashMap<String, StyledString>>>,
    sub_cache: Arc<Mutex<HashMap<String, Vec<crate::ci::ArtifactEntry>>>>,
    current_content: Arc<Mutex<StyledString>>,
) {
    let default_idx = viewer
        .default_entry(&entries)
        .and_then(|de| entries.iter().position(|e| e.name == de.name));

    siv.call_on_name(
        "ci_files",
        |v: &mut SelectView<crate::ci::ArtifactEntry>| {
            v.clear();
            // Pin the default entry (report.html) at position 0 with a friendlier name.
            if let Some(idx) = default_idx {
                v.add_item(" Test results report", entries[idx].clone());
            }
            for (i, entry) in entries.iter().enumerate() {
                if Some(i) == default_idx {
                    continue; // already pinned at top
                }
                let icon = if entry.is_dir { "▶" } else { " " };
                v.add_item(format!("{} {}", icon, entry.name), entry.clone());
            }
            // Report is at position 0 when present; always select it.
            if default_idx.is_some() {
                let _ = v.set_selection(0);
            }
        },
    );

    // Show the auto-selected entry's content (set_selection does not trigger
    // on_select, so we do it manually here).
    let Some(idx) = default_idx else { return };
    let Some(entry) = entries.get(idx) else {
        return;
    };
    if entry.is_dir {
        return;
    }

    // Cache hit: instant display.
    if let Some(cached) = cache.lock().unwrap().get(&entry.url).cloned() {
        *current_content.lock().unwrap() = cached.clone();
        siv.call_on_name("ci_content", |v: &mut TextView| {
            v.set_content(cached);
        });
        siv.call_on_name(
            "ci_content_scroll",
            |v: &mut ScrollView<NamedView<TextView>>| {
                v.set_offset(cursive::Vec2::new(0, 0));
            },
        );
        // Re-attach test sub-entries that were loaded in a previous visit.
        if let Some(subs) = sub_cache.lock().unwrap().get(&entry.url).cloned() {
            if !subs.is_empty() {
                append_sub_entries_to_list(siv, subs);
            }
        }
        return;
    }

    // Cache miss: fetch in background, then cache and display.
    let entry = entry.clone();
    let viewer2 = Arc::clone(&viewer);
    let cache2 = Arc::clone(&cache);
    let sub2 = Arc::clone(&sub_cache);
    let cc2 = Arc::clone(&current_content);
    let cb = siv.cb_sink().clone();
    std::thread::spawn(move || match viewer2.fetch_content(&entry) {
        Ok(crate::ci::ContentResult {
            content,
            sub_entries,
        }) => {
            cache2
                .lock()
                .unwrap()
                .insert(entry.url.clone(), content.clone());
            // Pre-load per-test log content and index the test artifacts.
            let sub_artifacts: Vec<crate::ci::ArtifactEntry> = sub_entries
                .into_iter()
                .map(|(artifact, log)| {
                    cache2.lock().unwrap().insert(artifact.url.clone(), log);
                    artifact
                })
                .collect();
            if !sub_artifacts.is_empty() {
                sub2.lock()
                    .unwrap()
                    .insert(entry.url.clone(), sub_artifacts.clone());
            }
            cb.send(Box::new(move |s: &mut Cursive| {
                *cc2.lock().unwrap() = content.clone();
                s.call_on_name("ci_content", |v: &mut TextView| {
                    v.set_content(content);
                });
                s.call_on_name(
                    "ci_content_scroll",
                    |v: &mut ScrollView<NamedView<TextView>>| {
                        v.set_offset(cursive::Vec2::new(0, 0));
                    },
                );
                if !sub_artifacts.is_empty() {
                    append_sub_entries_to_list(s, sub_artifacts);
                }
            }))
            .ok();
        }
        Err(e) => {
            cb.send(Box::new(move |s: &mut Cursive| {
                s.call_on_name("ci_content", |v: &mut TextView| {
                    v.set_content(format!("Error loading {}:\n{}", entry.name, e));
                });
            }))
            .ok();
        }
    });
}

/// Insert a test section immediately after "Test results report" (position 0)
/// in the `"ci_files"` SelectView.  A "Files" separator is placed between the
/// test list and the remaining file entries.  Called after a pytest report is
/// parsed; entries are already pre-loaded in the content cache.
fn append_sub_entries_to_list(siv: &mut Cursive, sub_artifacts: Vec<crate::ci::ArtifactEntry>) {
    siv.call_on_name(
        "ci_files",
        |v: &mut SelectView<crate::ci::ArtifactEntry>| {
            // "Test results report" sits at position 0.  We insert everything
            // at position 1 in reverse so the final layout is:
            //   0: Test results report
            //   1: ─── Tests ──────────
            //   2: test_a
            //   3: test_b
            //   …
            //   N: ─── Files ──────────
            //   N+1: runner.log.gz
            //   …

            // "Files" separator lands just above the existing file entries.
            v.insert_item(
                1,
                "─── Files ──────────────────────────────────────",
                crate::ci::ArtifactEntry {
                    name: String::new(),
                    url: "sep://files".to_string(),
                    is_dir: false,
                },
            );
            // Test entries inserted in reverse so the final order is preserved.
            for artifact in sub_artifacts.iter().rev() {
                v.insert_item(1, artifact.name.clone(), artifact.clone());
            }
            // "Tests" header goes right after "Test results report".
            v.insert_item(
                1,
                "─── Tests ──────────────────────────────────────",
                crate::ci::ArtifactEntry {
                    name: String::new(),
                    url: "sep://tests".to_string(),
                    is_dir: false,
                },
            );
        },
    );
}

/// Open a "File Bug" dialog pre-filled with the current right-pane content.
///
/// Requires a Pagure client in `TuiState`.  If one is not configured the
/// dialog is replaced by an informational message.  Submission is done in a
/// background thread so the TUI remains responsive; the dialog is dismissed
/// immediately and the result is reported via a follow-up info/error dialog.
fn show_file_bug_dialog(
    siv: &mut Cursive,
    job_name: &str,
    current_content: &Arc<Mutex<StyledString>>,
) {
    let pagure = match siv.user_data::<TuiState>().and_then(|t| t.pagure.clone()) {
        Some(p) => p,
        None => {
            show_info(siv, "Bug tracker (Pagure) is not configured.\nSet pagure-token and pagure-repository in your config.");
            return;
        }
    };

    // Pre-fill the title from the currently selected artifact entry name.
    let selected_name = siv
        .call_on_name(
            "ci_files",
            |v: &mut SelectView<crate::ci::ArtifactEntry>| {
                v.selection().map(|e| e.name.trim().to_string())
            },
        )
        .flatten()
        .unwrap_or_default();

    let default_title = if selected_name.is_empty() || selected_name.starts_with('─') {
        format!("CI failure: {}", job_name)
    } else {
        format!("CI failure: {} — {}", job_name, selected_name)
    };

    let body_arc = Arc::clone(current_content);

    siv.add_layer(
        Dialog::new()
            .title("File Bug Report")
            .content(
                LinearLayout::vertical()
                    .child(TextView::new("Title:"))
                    .child(
                        EditView::new()
                            .content(default_title)
                            .with_name("bug_title")
                            .min_width(64),
                    )
                    .child(TextView::new(
                        "\nThe content currently shown in the right pane will be\nused as the bug body.\n",
                    )),
            )
            .button("Cancel", |s| {
                s.pop_layer();
            })
            .button("File Bug", move |s| {
                let title = s
                    .call_on_name("bug_title", |v: &mut EditView| {
                        v.get_content().to_string()
                    })
                    .unwrap_or_default();
                let title = title.trim().to_string();
                if title.is_empty() {
                    return;
                }
                s.pop_layer();

                // Extract plain text from the StyledString.
                let body: String = body_arc
                    .lock()
                    .unwrap()
                    .spans()
                    .map(|sp| sp.content)
                    .collect();

                let pagure2 = Arc::clone(&pagure);
                let cb = s.cb_sink().clone();
                std::thread::spawn(move || match pagure2.create_issue(&title, &body) {
                    Ok(id) => {
                        cb.send(Box::new(move |s: &mut Cursive| {
                            show_info(s, &format!("Bug #{} filed successfully!", id));
                        }))
                        .ok();
                    }
                    Err(e) => {
                        cb.send(Box::new(move |s: &mut Cursive| {
                            show_error(s, &format!("Failed to file bug:\n{:#}", e));
                        }))
                        .ok();
                    }
                });
            }),
    );
}
