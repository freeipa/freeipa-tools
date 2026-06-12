# ipatool

A Rust port of the FreeIPA development workflow tool.  It automates the
repetitive parts of reviewing and landing patches: fetching patches, applying
them, pushing upstream, updating issue trackers (Pagure, Forgejo, Jira), and
managing GitHub pull requests.

## Building

Make sure you've installed non-Rust dependencies:
```sh
dnf install sqlite-devel
```

```sh
cargo build --release
# binary at target/release/ipatool
```

## Configuration

On first run, generate a commented sample configuration:

```
ipatool sample-config > ~/.ipa/toolconf.yaml
```

The default config path is `~/.ipa/toolconf.yaml`.  Override it with
`--config <path>`.

### Full configuration reference

```yaml
# ── Git repository ────────────────────────────────────────────────────────────

# Absolute path to the local checkout used for applying patches and pushing.
clean-repo-path: ~/dev/freeipa-clean

# Name of the upstream git remote (default: origin).
remote: origin

# Directory where downloaded patch files are stored temporarily.
patchdir: ~/patches/to-apply

# ── URL templates ─────────────────────────────────────────────────────────────

# Base URL for Pagure/Forgejo issues.  The issue number is appended.
ticket-url: https://pagure.io/freeipa/issue/

# Base URL for upstream commit links included in comments.
commit-url: https://pagure.io/freeipa/c/

# Base URL for Bugzilla bugs (appended with bug ID).
bugzilla-bug-url: https://bugzilla.redhat.com/show_bug.cgi?id=

# Base URL for Jira tickets (appended with key).  The server URL is derived
# automatically from the path prefix before "/browse/".
jira-ticket-url: https://issues.redhat.com/browse/RHEL-

# ── Pagure (primary issue tracker) ───────────────────────────────────────────
# Use either Pagure or Forgejo — not both.

pagure-repository: freeipa
# Create at https://pagure.io/<repo>/settings#apikeys
# Required permissions: assign/change status/comment/create/subscribe/update
# issues; create issues; update custom fields; update milestone.
pagure-token: "0123456789abcdef0123456789abcdef01234567"

# ── Forgejo (alternative issue tracker) ──────────────────────────────────────

# forgejo-url: https://forgejo.example.com
# forgejo-repo: owner/repository
# forgejo-token: "0123456789abcdef0123456789abcdef01234567"

# ── Issue tracker operations ──────────────────────────────────────────────────
# Values: yes | no | ask  (ask = prompt interactively each time)

update-issue: ask   # post a push-summary comment on the issue
close-issue:  ask   # close the issue after a successful push

# ── Jira (secondary tracker) ──────────────────────────────────────────────────
# Jira ticket keys are read from the 'rhbz' custom field of Pagure/Forgejo
# issues and matched against jira-ticket-url above.

# jira-token: "your-personal-access-token"
# update-jira: ask   # post commit-info comment on the Jira issue
# close-jira:  no    # transition Jira issue (yes/no/ask)
# jira-close-transition: Fixed   # transition name to use when closing

# ── GitHub pull requests ──────────────────────────────────────────────────────
# Required token scopes: repo, admin:org, user:email, read:user

gh-token: "0123456789abcdef0123456789abcdef01234567"
gh-repo: "freeipa/freeipa"
# Remote name in your local git checkout that points to your personal fork.
# Used when creating backport pull requests.
gh-fork-remote: "mygh"

# ── git am command ────────────────────────────────────────────────────────────
# Argv list used by the 'am' subcommand to apply patches on a remote machine.

am-command: ["ssh", "ipa-devel-vm.local", "cd ~/freeipa/ ; git am -3"]

# ── Username mapping ──────────────────────────────────────────────────────────
# Maps Pagure/Forgejo logins to the "Name <email>" format used in
# "Reviewed-By:" git trailers.

trac-username-map:
    abbra: Alexander Bokovoy <abokovoy@redhat.com>

# ── Local cache database ──────────────────────────────────────────────────────
# SQLite database used for offline mode (default shown).

db-path: ~/.ipa/ipatool-cache.db

# ── Issue tracker migration (e.g. pagure → Codeberg) ─────────────────────────
# When ticket-url has been updated to the new tracker but existing commits still
# contain the old URL prefix, set legacy-ticket-url.  Issue numbers found via
# this prefix are treated exactly like those found via ticket-url.

# legacy-ticket-url: "https://pagure.io/freeipa/issue/"

# If the migration did not preserve issue numbers, map old numbers to new ones.
# Unmapped numbers pass through unchanged.

# issue-number-map:
#   9000: 1234   # pagure #9000 → Codeberg #1234
#   8999: 1233

# When true, legacy-ticket-url references in commit messages are replaced with
# ticket-url before git-am writes them into history on push/backport.
# Defaults to false (old pagure URLs are preserved in the commit log).

# rewrite-ticket-urls: false

# ── Named profiles ────────────────────────────────────────────────────────────
# A profile overrides selected top-level fields and sets the PR source and
# issue tracker.  Activate with --profile <name>.
#
# pr-source     : github | forgejo | pagure  (default: github)
# issue-tracker : pagure | forgejo | github  (default: pagure)
#
# Any connection field (gh-*, pagure-*, forgejo-*, ticket-url, commit-url,
# db-path) can be overridden per-profile; unset fields inherit the top-level
# value.

profiles:
  # explicit default — same as omitting --profile
  default:
    pr-source: github
    issue-tracker: pagure

  # (1) GitHub PRs + Codeberg issues
  gh-cb-issues:
    pr-source: github
    issue-tracker: forgejo
    forgejo-url: https://codeberg.org
    forgejo-repo: myuser/freeipa
    forgejo-token: "YOUR_CODEBERG_TOKEN_HERE"
    ticket-url: https://codeberg.org/myuser/freeipa/issues/

  # (2) Codeberg PRs + Codeberg issues
  codeberg:
    pr-source: forgejo
    issue-tracker: forgejo
    forgejo-url: https://codeberg.org
    forgejo-repo: myuser/freeipa
    forgejo-token: "YOUR_CODEBERG_TOKEN_HERE"
    ticket-url: https://codeberg.org/myuser/freeipa/issues/

  # (3) Codeberg PRs + Pagure issues
  cb-pagure:
    pr-source: forgejo
    issue-tracker: pagure
    forgejo-url: https://codeberg.org
    forgejo-repo: myuser/freeipa
    forgejo-token: "YOUR_CODEBERG_TOKEN_HERE"

  # (4) GitHub PRs + GitHub issues
  gh-issues:
    pr-source: github
    issue-tracker: github

  # (5) Codeberg PRs + Codeberg issues, with pagure legacy URL support
  #     (commits written against pagure are still recognised)
  codeberg-migrated:
    pr-source: forgejo
    issue-tracker: forgejo
    forgejo-url: https://codeberg.org
    forgejo-repo: freeipa/freeipa
    forgejo-token: "YOUR_CODEBERG_TOKEN_HERE"
    ticket-url: https://codeberg.org/freeipa/freeipa/issues/
    legacy-ticket-url: "https://pagure.io/freeipa/issue/"
```

### Named profiles

The `profiles:` section lets you switch the PR source and issue tracker without
maintaining separate config files.

```
ipatool --profile codeberg tui         # Codeberg PRs, Codeberg issues
ipatool --profile cb-pagure pr-list    # Codeberg PRs, Pagure issues
ipatool --profile gh-issues pr-list    # GitHub PRs, GitHub issues (no Pagure)
```

**Supported combinations**

| Profile example | PR source | Issue tracker | Notes |
|-----------------|-----------|---------------|-------|
| `default` (no flag) | GitHub | Pagure | Original workflow |
| `gh-cb-issues` | GitHub | Codeberg | PRs stay on GitHub, bugs on Codeberg |
| `codeberg` | Codeberg | Codeberg | Full Codeberg / Forgejo workflow |
| `cb-pagure` | Codeberg | Pagure | Codeberg PRs, Pagure issue tracker |
| `gh-issues` | GitHub | GitHub Issues | No Pagure/Forgejo needed |
| `codeberg-migrated` | Codeberg | Codeberg | Codeberg workflow; old pagure commit URLs still recognised |

**Profile fields**

| Field | Values | Description |
|-------|--------|-------------|
| `pr-source` | `github`, `forgejo`, `pagure` | Where pull requests are fetched from |
| `issue-tracker` | `pagure`, `forgejo`, `github` | Which tracker holds linked tickets |
| `gh-token`, `gh-repo`, `gh-fork-remote` | strings | Override GitHub settings |
| `pagure-repository`, `pagure-token` | strings | Override Pagure settings |
| `forgejo-url`, `forgejo-repo`, `forgejo-token` | strings | Override Forgejo/Codeberg settings |
| `ticket-url`, `commit-url`, `db-path` | strings | Override URL templates and cache path |
| `legacy-ticket-url` | string | Old ticket URL prefix to recognise in existing commits (migration) |

Profile field values replace their top-level counterparts when the profile is
active; omitted fields fall through to the top-level values.

When `--profile` is omitted, `pr-source` defaults to `github` and
`issue-tracker` defaults to `pagure`.

**Per-profile cache isolation**

The local SQLite cache is namespaced by profile name.  Running
`cache-update` or the TUI under `--profile codeberg` populates a completely
separate cache from the default (no `--profile`) session, even if both
profiles happen to use the same forge.  This means:

- `ipatool cache-update` — populates the default profile's cache.
- `ipatool --profile codeberg cache-update` — populates the `codeberg`
  profile's cache; does not affect the default cache.
- `ipatool --offline tui` will not see PRs cached under `--profile codeberg`,
  and vice versa.

On the first run after upgrading from a version that did not support
per-profile caching the old cache tables are dropped and recreated
automatically.  Run `cache-update` again to repopulate.

**Notes on Forgejo/Codeberg PR support**

When `pr-source: forgejo` is active:
- PRs are listed, browsed, ACKed, and rejected using the Forgejo API.
- Labels are managed by name; missing labels are created automatically.
- Inline review comments fall back to regular issue comments (Forgejo's review API differs from GitHub's).
- Patch download for `pr-push` uses the Forgejo web endpoint `{base-url}/{owner}/{repo}/commit/{sha}.patch`.

When `issue-tracker: github` is active:
- Issue operations (comment, close) use the GitHub Issues API.
- GitHub Issues have no `reviewer` or `rhbz` custom fields; those are treated as absent.
- Forgejo issues also have no custom fields; ipatool reads them from issue comments (see [Forgejo comment-based custom fields](#forgejo-comment-based-custom-fields)).

## Global flags

These flags apply to every subcommand.

| Flag | Default | Description |
|------|---------|-------------|
| `--config <path>` | `~/.ipa/toolconf.yaml` | Configuration file |
| `--profile <name>` | — | Named profile from the `profiles:` config section |
| `-v` / `-vv` / `-vvv` | off | Increase verbosity |
| `-n`, `--dry-run` | off | Skip actual git push (still applies patches locally) |
| `--no-reviewer` | off | Omit the `Reviewed-By:` trailer |
| `--no-fetch` | off | Skip `git fetch` before applying patches |
| `--no-pagure` | off | Disable all Pagure API calls |
| `--no-forgejo` | off | Disable all Forgejo API calls |
| `--no-jira` | off | Disable all Jira API calls |
| `--color <mode>` | `auto` | Terminal color: `auto`, `always`, `never` |
| `--offline` | off | Use local cache; queue mutations for later sync |

---

## Subcommands

### `sample-config`

Print a fully commented sample configuration file to stdout.

```
ipatool sample-config
ipatool sample-config > ~/.ipa/toolconf.yaml
```

---

### `push`

Apply one or more patch files (or directories of patch files) to the local
clean repository and push them upstream.  For each branch the reviewer trailer
is added, the patch is applied with `git am`, and pushed via `git push`.

After a successful push, ipatool can optionally:
- post a comment on the linked Pagure/Forgejo issue,
- close that issue,
- post a comment on the linked Jira issue,
- transition that Jira issue.

Interactive prompts are controlled by `update-issue`, `close-issue`,
`update-jira`, and `close-jira` in the config.

```
ipatool push -b master -r abbra ~/patches/0001-fix-something.patch
ipatool push -b master -b ipa-4-12 -r abbra ~/patches/
ipatool push --dry-run -b master -r abbra ~/patches/0001-fix.patch
```

**Flags**

| Flag | Description |
|------|-------------|
| `-b`, `--branch <branch>` | Target branch (repeatable; inferred from ticket milestone if omitted) |
| `-r`, `--reviewer <name>` | Reviewer login or full name (repeatable) |

Reviewer names are resolved against `git shortlog` of the upstream branch.
Partial matches work (e.g. `abbra` → `Alexander Bokovoy <abokovoy@redhat.com>`).
The `trac-username-map` config key provides explicit overrides.

---

### `start-review`

Set yourself as the reviewer on one or more Pagure/Forgejo tickets and
optionally apply the patches to a development VM.

```
ipatool start-review -t 8309 ~/patches/0001-fix.patch
ipatool start-review --am -t 8309 -t 8310 ~/patches/
ipatool start-review --force -t 8309   # re-assign even if already set
```

**Flags**

| Flag | Description |
|------|-------------|
| `-t`, `--ticket <num>` | Issue number (repeatable) |
| `-f`, `--force` | Re-assign reviewer even if the field is already set |
| `--am` | Also apply patches via `am-command` (like running `ipatool am`) |

---

### `am`

Apply patches to the remote development tree using the `am-command` from the
config.  Typically this SSH's into a dev VM and runs `git am -3`.

```
ipatool am ~/patches/0001-fix.patch
ipatool am ~/patches/
```

**Arguments:** one or more patch files or directories.

---

### `pr-list`

List GitHub pull requests with optional state and label filters.  After
listing, also scans the last 100 recently-updated PRs for common mistakes
(merged without `pushed` label, pushed without `ack` label).

Results are also written to the local SQLite cache so the TUI and offline mode
have fresh data without a separate `cache-update` run.

```
ipatool pr-list                       # open PRs
ipatool pr-list -s closed             # closed PRs
ipatool pr-list -s all -l ack         # all PRs labelled 'ack'
ipatool pr-list -s all -l -rejected   # all PRs NOT labelled 'rejected'
```

Output columns (tab-separated): `number  title  labels  URL  CI-statuses`

**Flags**

| Flag | Description |
|------|-------------|
| `-s`, `--state <state>` | Filter by state: `open`, `closed`, `all`. Prefix with `-` to exclude. Repeatable. |
| `-l`, `--label <label>` | Filter by label name. Prefix with `-` to exclude. Repeatable. |

---

### `pr-ack`

Add the `ack` label to a PR, remove `rejected` if present, and optionally
post a comment.

```
ipatool pr-ack 8309
ipatool pr-ack 8309 -c "LGTM, thanks"
ipatool --offline pr-ack 8309 -c "LGTM"   # queued for later sync
```

**Arguments:** `<pr_id>` (required)

**Flags**

| Flag | Description |
|------|-------------|
| `-c`, `--comment <text>` | Comment text (optional) |

In offline mode (`--offline`) the action is queued in the local database and
applied when connectivity is restored via `queue-submit` or the TUI `s` key.

---

### `pr-reject`

Add the `rejected` label, remove `ack` if present, post a mandatory comment,
and close the PR.

```
ipatool pr-reject 8309 -c "Needs rebase on top of master"
ipatool --offline pr-reject 8309 -c "Needs rebase"   # queued
```

**Arguments:** `<pr_id>` (required)

**Flags**

| Flag | Description |
|------|-------------|
| `-c`, `--comment <text>` | Rejection reason (required) |

---

### `pr-push`

Full push pipeline for a GitHub PR:

1. Validates the PR (ACKed, not rejected, not already pushed, CI green,
   mergeable).
2. Downloads commits as patch files into `patchdir`.
3. Runs the `push` pipeline (apply + push to `base` branch of the PR).
4. Adds the `pushed` label, posts a push-summary comment, and closes the PR.
5. Optionally creates backport PRs (auto-ACKed, CI-pending).

```
ipatool pr-push 8309 -r abbra
ipatool pr-push 8309 -r abbra -B ipa-4-12
ipatool pr-push 8309 -r abbra --autobackport   # use labels like 'ipa-4-12'
```

**Arguments:** `<pr_id>` (required)

**Flags**

| Flag | Description |
|------|-------------|
| `-r`, `--reviewer <name>` | Reviewer name (repeatable) |
| `-B`, `--backport <branch>` | Backport target branch (repeatable) |
| `--autobackport` | Auto-detect backport branches from PR labels matching `ipa-N-N` |

---

### `backport`

Backport the patches from an already-ACKed PR to one or more older branches.
Downloads the PR commits as patches, applies them to each branch, pushes to
the personal fork remote, and opens a new auto-ACKed PR against the target
branch.

```
ipatool backport 8309 -b ipa-4-12
ipatool backport 8309 -b ipa-4-12 -b ipa-4-11
```

**Arguments:** `<pr_id>` (required)

**Flags**

| Flag | Description |
|------|-------------|
| `-b`, `--branch <branch>` | Target branch (required, repeatable) |

If patch application fails for a branch, a warning is printed and the tool
continues with the remaining branches.

---

### `tui`

Interactive terminal UI for browsing, reviewing, and acting on GitHub pull
requests.  The TUI remains open between operations — after a push or backport
it returns you to the PR list showing the updated state.

```
ipatool tui                  # open PRs (default)
ipatool tui --state closed
ipatool tui --state all
ipatool --offline tui        # use cached data (no network)
```

**Flags**

| Flag | Default | Description |
|------|---------|-------------|
| `--state` | `open` | Which PRs to load: `open`, `closed`, `all` |

#### Startup behaviour

- **Online, no cache:** a loading spinner is shown while PRs are fetched from
  GitHub.
- **Online, cache available:** cached PRs are displayed immediately so the TUI
  is interactive at once.  A background thread fetches fresh data and swaps the
  list in silently when it arrives.  On network error the cached view stays and
  the user can press `r` to retry.
- **Offline:** PRs and full detail data (CI status, comments, changed files) are
  served entirely from the local SQLite cache populated by `cache-update` or
  previous online sessions.

#### Layout

```
  a:ACK  x:Reject  c:Review  b:Browser  r:Refresh  q:Quit  Tab:Focus  j/↓:Down  k/↑:Up  d/u:Scroll  i:Inspect  Enter:Actions
┌── 47 PRs ────────────────────┐┌── Details ──────────────────────────────────┐
│ #8309 ○ Fix LDAP timeout  …  ││ PR #8309: Fix LDAP connection timeout        │
│ #8308 ○ Add KDC support   …  ││                                              │
│ #8307 ✓ Refactor tests [ack] ││ URL:    https://github.com/…/pull/8309       │
│ #8306 ✗ Old fix [rejected]   ││ State:  open  |  Author: @contributor        │
│ …                            ││ Labels: needs-rebase                         │
│                              ││ Base:   master                               │
│                              ││                                              │
│                              ││ CI Status:                                   │
│                              ││   ✓ ci/freeipa  [i:inspect]                  │
│                              ││   ✗ ci/lint     [i:inspect]                  │
│                              ││                                              │
│                              ││ Changed files (3 files, +42 / -7):           │
│                              ││   M  ipaserver/plugins/ldap2.py  (+38 / -5)  │
│                              ││                                              │
│                              ││ Description:                                 │
│                              ││   Fix the LDAP connection timeout by …       │
│                              ││                                              │
│                              ││ Comments (2):                                │
│                              ││ @reviewer [2024-03-01]:                      │
│                              ││   LGTM, one nit below                        │
│                              ││ ────────────────────────────────────────     │
│                              ││ @author [2024-03-02]:                        │
│                              ││   Thanks, fixed in the next revision         │
└──────────────────────────────┘└─────────────────────────────────────────────┘
```

The active pane's title is highlighted in **cyan**; the inactive pane's title
is plain.  Press `Tab` to toggle focus between the two panes.

When the detail pane has focus, the help bar shows `[Detail]` in cyan and
`j`/`↓`/`k`/`↑` scroll the detail pane instead of moving the PR selection:

```
  [Detail]  a:ACK  x:Reject  …  Tab:Focus  j/↓:Scroll  k/↑:Scroll  d/u:Scroll
```

In offline mode the help bar shows instead:
```
[OFFLINE | 3 queued | s:Sync]  a:ACK  x:Reject  …
```
The offline tag is shown in yellow.

The left pane colour-codes PRs: green = acked, red = rejected/closed,
default = pending.  The right pane refreshes in the background as you move
between PRs.  The right pane shows the full PR description and all issue
comments, both rendered as Markdown (blockquotes, code blocks, bold/italic,
links, lists).  Comments are separated by a horizontal rule; the author/date
header is flush-left and the body is indented by two spaces.  Use `d`/`u` or
`Tab` then `j`/`k` to scroll the detail pane without changing the selected PR.

Completed CI jobs that have a stored result URL show a dim `[i:inspect]` hint
next to their status line.  Press `i` from either pane to open the
[CI job results viewer](#ci-job-results-viewer-i).

#### Default keyboard shortcuts

All keys listed below are the defaults.  Every key can be remapped in
`~/.ipa/toolconf-keys.yaml` (see [TUI customisation](#tui-customisation)).

**Main PR list**

| Key | Action |
|-----|--------|
| `Tab` | Toggle keyboard focus between the PR list (left) and detail pane (right) |
| `j` / `↓` | Move PR selection down (left focus) or scroll detail pane down (right focus) |
| `k` / `↑` | Move PR selection up (left focus) or scroll detail pane up (right focus) |
| `d` | Scroll detail pane down 5 lines (works from either focus) |
| `u` | Scroll detail pane up 5 lines (works from either focus) |
| `Enter` | Open action menu for selected PR |
| `a` | ACK selected PR |
| `x` | Reject selected PR |
| `c` | Open diff review for selected PR |
| `b` | Open PR in browser |
| `i` | Open CI job results viewer (only available for PRs with completed CI jobs) |
| `r` | Refresh PR list from GitHub |
| `s` | Sync queued offline actions to GitHub |
| `q` | Quit |

#### Action menu (`Enter`)

Opened with `Enter` on a PR.  Available actions depend on PR state.

| Button / key | Available when | Action |
|--------------|---------------|--------|
| Review `v` | always | Show diff and comments |
| Labels `l` | always | Add / remove labels |
| ACK `a` | not acked, not rejected, not closed | ACK the PR |
| Reject `x` | not rejected, not closed | Reject and close the PR |
| Push `p` | acked, not pushed, not rejected, not closed | Push pipeline (quits TUI, runs in terminal, returns) |
| Backport `B` | acked, not rejected | Backport to branches (quits TUI, runs in terminal, returns) |
| Browser `b` | always | Open in browser |
| Close `Esc` | always | Dismiss menu |

#### Review view (`c` or `v`)

Shows the unified diff of all changed files with inline review comments.
Old and new line numbers are shown in the gutter.

| Key | Action |
|-----|--------|
| `j` / `↓` | Scroll down |
| `k` / `↑` | Scroll up |
| `c` / `Enter` | Open comment form at selected line |
| `q` / `Esc` | Close review view |

#### Comment form

Split view: diff (scrolled to the selected line) on top, form on the bottom.
Pre-fills file path and line number from the selected diff line.

| Field | Description |
|-------|-------------|
| File path | Path of the file to comment on |
| Line number | Line in the new (right) file |
| Comment | Multi-line comment body (Tab inserts newline) |

| Button / key | Action |
|-------------|--------|
| Post | Post comment (queued if offline) |
| Cancel / `Esc` | Discard and return to review view |

#### Label editor (`l`)

Split view: label checkboxes (left) and PR summary (right).
Space toggles a label; Tab moves to the next checkbox.
Apply / Cancel / `q` / `Esc` close the editor.  In offline mode, changes are queued.

#### Push form (`p`)

Collects reviewer(s), optional backport branches, and the autobackport flag,
then quits the TUI, runs `pr-push` in the terminal (with full output), and
returns you to the TUI afterwards.

#### Backport form (`B`)

Collects target branches (comma-separated), then quits the TUI, runs the
backport pipeline in the terminal, and returns.

#### CI job results viewer (`i`)

Press `i` from any PR list focus to drill into the CI artifacts for the
selected PR.  The key is only active when at least one CI job has a stored
result URL (i.e. its status is not `pending`).

If the PR has more than one such job a **job selector** dialog appears first.
Select a job and press Enter to open its artifact browser.

The viewer opens as a two-pane fullscreen layer:

```
 ↓/↑:Navigate  PgUp/PgDn:Page  Home/End:Jump  Tab:Switch Pane  Enter:Open  Backspace:Parent  b:Bug  q/Esc:Close
┌── Artifacts ───────────────────────────────────┐┌── Content (ci/freeipa) ────────────────────────┐
│  Test results report                            ││ ✗ 3 Failed  ✓ 142 Passed  – 1 Skipped         │
│ ─── Tests ─────────────────────────────────────││ ──────────────────────────────────────────     │
│ ✗ test_ldap::test_connection_timeout            ││ Failed                                         │
│ ✓ test_ldap::test_search                        ││   ✗ test_ldap::test_connection_timeout  1.23s  │
│ – test_ldap::test_skip                          ││   ✗ test_api::test_permission             0.9s  │
│ …                                               ││   ✗ test_dns::test_forward               0.5s  │
│ ─── Files ──────────────────────────────────────││ ──────────────────────────────────────────     │
│ ▶ runner/                                       ││ Passed  (142)                                  │
│   runner.log.gz                                 ││   ✓ test_ldap::test_search               0.1s  │
│   report.html                                   ││   …                                            │
└─────────────────────────────────────────────────┘└────────────────────────────────────────────────┘
```

**Left pane — artifact list:**

- `report.html` (if present) is pinned at the top under the label
  **Test results report**.
- If `report.html` is a pytest HTML report, its individual test cases are
  inserted below a `─── Tests ───` separator; each entry is prefixed with
  `✓`, `✗`, or `–` matching its result.  Selecting a test entry shows that
  test's log in the right pane.
- All other files and directories follow under a `─── Files ───` separator.
  Directories are prefixed with `▶`.

**Right pane — content:**

- `report.html`: parsed and rendered as a summary — total counts by result
  (Failed / Passed / Skipped / Error), then individual test rows with ID and
  duration.  Failed tests are highlighted in red, passed in green, skipped in
  yellow.
- `.gz` files: decompressed transparently and shown as plain text.
- Other files: raw text as-is.
- Directories: placeholder text; press Enter to navigate in.

Artifact content and directory listings are cached in memory for the session
so navigating back to a previously viewed file is instant.

**Keyboard shortcuts in the CI viewer:**

| Key | Action |
|-----|--------|
| `Tab` | Toggle keyboard focus between artifact list (left) and content (right); active pane title turns cyan |
| `↓` / `↑` | Move artifact selection (left focus) or scroll content (right focus) |
| `PgDn` / `PgUp` | Jump 10 items / lines |
| `End` / `Home` | Jump to last / first item (left) or bottom / top of content (right) |
| `Enter` | Navigate into directory (left focus, directory selected) |
| `Backspace` | Navigate to parent directory |
| `b` | Open "File Bug" dialog using the current right-pane content as the bug body (requires Pagure configured) |
| `q` / `Esc` | Close the viewer and return to the PR list |

**Filing a bug (`b`):**

Pressing `b` opens a small dialog pre-filled with a title derived from the
job name and the currently selected artifact entry.  On confirmation the
plain text of the right pane is submitted as the bug body via the Pagure API.
The dialog dismisses immediately; success or failure is reported in a
follow-up info/error dialog.

Requires `pagure-token` and `pagure-repository` (or `forgejo-token`,
`forgejo-url`, and `forgejo-repo`) to be set in the configuration, and the
token needs **Create issues** permission.

---

### `cache-update`

Fetch pull requests and store them — together with their CI statuses, all
issue comments, PR commits, and changed-file lists — in the local SQLite
cache.  Subsequent calls are incremental: only PRs whose `updated_at`
timestamp has changed since the last fetch have their details re-fetched.

The cache is namespaced by the active profile, so running `cache-update`
under different `--profile` values populates independent caches.

```
ipatool cache-update                          # cache open PRs (default profile)
ipatool cache-update --state all              # cache all PRs (open + closed)
ipatool --profile codeberg cache-update       # cache Codeberg PRs separately
```

**Flags**

| Flag | Default | Description |
|------|---------|-------------|
| `--state` | `open` | Which PRs to cache: `open`, `closed`, `all` |

Typical output:

```
Fetching open pull requests…
Cached 52 pull request(s).
Fetching details for PR #8309 (1/52)…
Details: 3 updated, 49 unchanged (skipped).
```

---

### `queue-list`

List all pending offline actions stored in the local SQLite database without
executing them.  Useful for reviewing what will be sent when connectivity is
restored.

```
ipatool queue-list
```

Example output:
```
2 queued action(s):
  [1] (github) ACK PR #8309
  [2] (github) Reject PR #8310
```

The provider shown in parentheses (`github` or `forgejo`) reflects the
forge that was active when the action was queued, ensuring `queue-submit`
dispatches to the correct API.

---

### `queue-submit`

Replay all queued offline actions against the live GitHub API, in the order
they were recorded.  Successfully applied actions are removed from the queue.
The first failure halts the queue so ordering is preserved.

```
ipatool queue-submit
```

Actions that can be queued and submitted:

| Action | Source |
|--------|--------|
| ACK PR | `--offline pr-ack` or TUI offline ACK dialog |
| Reject PR | `--offline pr-reject` or TUI offline reject dialog |
| Update labels | TUI label editor (offline) |
| Post review comment | TUI comment form (offline review view) |

---

## Offline mode

Run `ipatool --offline <subcommand>` to work without network access.

### Workflow

```bash
# While online: populate the cache
ipatool cache-update

# Go offline
ipatool --offline tui
```

In offline mode the TUI:
- loads the PR list and full detail pane (CI status, all comments, changed
  files) from the local SQLite database,
- queues mutations (ACK, reject, label changes, review comments) in the
  database instead of executing them immediately,
- shows `[OFFLINE | N queued | s:Sync]` in yellow in the help bar.

Details for PRs viewed in an online session are cached automatically, so
revisiting them offline shows the full information.

### Syncing queued actions

In the TUI, press `s` to replay the queue against GitHub.  From the command
line, use `queue-submit` (or inspect the queue first with `queue-list`).

Actions are replayed in order; the first failure halts the queue so ordering
is preserved.  Successfully replayed actions are deleted from the queue.

### What is cached

| Data | Cached when |
|------|------------|
| PR list (all fields) | `cache-update`, online TUI load, or `pr-list` |
| CI statuses, all comments, changed files, commits | `cache-update` or when a PR is selected in the online TUI |
| Mutations (ACK, reject, labels, comments) | Immediately, when performed in offline mode |

All cache entries are scoped to the active profile.  Data cached under
`--profile codeberg` is invisible to the default profile and vice versa.
Each profile must be populated independently with `cache-update`.

---

## TUI customisation

The TUI appearance and keybindings are each controlled by a YAML file that is
written with annotated defaults on first run.  Edit either file and restart the
TUI for changes to take effect.

### Appearance — `~/.ipa/toolconf-style.yaml`

(Derived from the main config path by replacing the extension with `-style.yaml`.)

```yaml
# Colors: default, dark-black, dark-red, dark-green, dark-yellow, dark-blue,
#   dark-magenta, dark-cyan, dark-white, light-black, light-red, light-green,
#   light-yellow, light-blue, light-magenta, light-cyan, light-white
#
# Borders: none, simple, outset

shadow: false
borders: simple

# Selection highlight color (focused and unfocused panels)
highlight: dark-blue
highlight-inactive: dark-blue

# Terminal palette colors ("default" inherits the terminal's own colors)
background: default
view: default
primary: default
title-primary: default
```

Any missing field falls back to the compiled-in default.  Parse errors warn to
stderr and the TUI starts with defaults rather than crashing.

### Keybindings — `~/.ipa/toolconf-keys.yaml`

(Derived from the main config path by replacing the extension with `-keys.yaml`.)

Each field is a single character.  All keys listed in the table below are
independent, so the same character can be reused across different views without
conflict (the views are on separate layers).

```yaml
# Main two-pane view
quit:        q
ack:         a
reject:      x
browser:     b
refresh:     r
review:      c
sync:        s
inspect:     i
down:        j
up:          k
scroll-down: d
scroll-up:   u

# Code review diff view
review-comment: c
review-down:    j
review-up:      k
review-close:   q

# Action dialog
action-review:   v
action-labels:   l
action-push:     p
action-backport: B
```

Any missing field falls back to the compiled-in default shown above.

---

## Tracker integration

### Pagure / Forgejo

Used as the primary issue tracker.  After a push, ipatool reads `ticket-url`
links from commit messages to identify associated issues, then optionally:

- posts a comment with the push summary (controlled by `update-issue`),
- closes the issue (controlled by `close-issue`).

Only one tracker (Pagure or Forgejo) is active at a time.  Disable the other
with `--no-pagure` or `--no-forgejo`.

### Jira

Used as a secondary tracker.  Jira ticket keys are read from the `rhbz` custom
field of each Pagure issue, or from issue comments for Forgejo (see
[Forgejo comment-based custom fields](#forgejo-comment-based-custom-fields)).
After a push, ipatool optionally:

- posts a commit-info comment (`update-jira`),
- transitions the issue (`close-jira`, using the `jira-close-transition` name).

The Jira REST API server is derived automatically from the scheme+host of
`jira-ticket-url`.  Disable Jira with `--no-jira`.

### Issue tracker migration (pagure → Codeberg)

When the project's primary tracker changes (e.g. from pagure.io to
codeberg.org), existing commits continue to reference the old URL.  Three
config keys handle the transition without requiring a rebase of every branch:

#### `legacy-ticket-url`

Set this to the **old** issue URL prefix.  ipatool scans commit messages for
both `ticket-url` and `legacy-ticket-url` when extracting issue numbers from
patches.  Numbers found via either URL are used identically to look up, comment
on, and close the corresponding issues in the **new** tracker.

```yaml
ticket-url: "https://codeberg.org/freeipa/freeipa/issues/"
legacy-ticket-url: "https://pagure.io/freeipa/issue/"
```

With this configuration, a commit that contains:

```
Fixes: https://pagure.io/freeipa/issue/9000
```

is treated as if it referenced issue `#9000` in the Codeberg tracker and the
Codeberg issue is commented on / closed after a push.

#### `issue-number-map`

If the migration did **not** preserve issue numbers (pagure issue `#9000` became
Codeberg issue `#1234`), provide an explicit mapping.  Unmapped numbers pass
through unchanged.

```yaml
issue-number-map:
  9000: 1234
  8999: 1233
```

The map is applied after all ticket numbers have been extracted (from both
`ticket-url` and `legacy-ticket-url`), so pagure numbers found via
`legacy-ticket-url` are translated to their Codeberg counterparts before any
API call is made.

#### `rewrite-ticket-urls`

When set to `true`, any `legacy-ticket-url` occurrence in patch commit messages
is replaced with `ticket-url` **before** `git am` writes the commit into the
target branch.  This produces a clean commit history on the server side — the
pushed commits contain only the new Codeberg URL.

```yaml
rewrite-ticket-urls: true
```

The rewrite is applied only to the patch header (commit message lines).  The
diff body is never modified.  The setting has no effect when `legacy-ticket-url`
is empty or equal to `ticket-url`.

Default is `false` (the original pagure URL is preserved verbatim in the commit
log).

#### Minimal migration config

```yaml
# New primary tracker
ticket-url: "https://codeberg.org/freeipa/freeipa/issues/"
commit-url: "https://codeberg.org/freeipa/freeipa/commit/"
forgejo-url: "https://codeberg.org"
forgejo-repo: "freeipa/freeipa"
forgejo-token: "YOUR_CODEBERG_TOKEN_HERE"

# Legacy pagure tracker (read-only; used only to recognise old commit URLs)
legacy-ticket-url: "https://pagure.io/freeipa/issue/"

# Rewrite pagure URLs to Codeberg URLs in pushed commits (optional)
rewrite-ticket-urls: true

profiles:
  codeberg:
    pr-source: forgejo
    issue-tracker: forgejo
```

Activate the Codeberg workflow with:

```
ipatool --profile codeberg pr-push 8309 -r abbra
```

### Forgejo comment-based custom fields

Pagure issues supported **custom fields** — in particular `rhbz` (downstream
Bugzilla/Jira links) and `reviewer` (the assigned reviewer login).  Forgejo has
no equivalent.  ipatool recovers this information by scanning issue comments for
specially formatted lines.

#### Comment format

Each metadata line placed anywhere in any issue comment must follow the pattern:

```
<prefix><fieldname>: <value>
```

**With `forgejo-comment-field-prefix: "ipatool:"`:**

```
ipatool:rhbz: https://bugzilla.redhat.com/show_bug.cgi?id=12345
ipatool:rhbz: https://issues.redhat.com/browse/RHEL-99
ipatool:reviewer: abbra
```

Multiple `rhbz:` lines are merged so that both the Bugzilla and Jira URLs are
visible to the downstream-update logic (which already extracts several URLs from
a single `rhbz` value).

**With an empty prefix (the default):**

```
rhbz: https://bugzilla.redhat.com/show_bug.cgi?id=12345
reviewer: abbra
```

#### Configuration

```yaml
# Optional prefix prepended to field names in comments.
# Leave empty (or omit) to match bare "rhbz: …" / "reviewer: …" lines.
forgejo-comment-field-prefix: "ipatool:"
```

The prefix can also be overridden per profile:

```yaml
profiles:
  codeberg:
    pr-source: forgejo
    issue-tracker: forgejo
    forgejo-comment-field-prefix: "ipatool:"
```

---

## Milestone-to-branch mapping

When no `--branch` is specified, `push` infers target branches from the
milestone of the linked issue:

| Milestone pattern | Branches |
|-------------------|----------|
| `FreeIPA 3.3.*` | `master`, `ipa-4-1`, `ipa-4-0`, `ipa-3-3` |
| `FreeIPA 4.4*` | `master`, `ipa-4-5`, `ipa-4-4` |
| `FreeIPA 4.5*` | `master`, `ipa-4-5` |
| `FreeIPA 4.6*` | `master` |
| `FreeIPA 4.7*` | `master` |

---

## Typical workflow

```bash
# 1. Reviewer picks up a PR
ipatool start-review -t 8309 ~/patches/0001-fix.patch

# 2. Review the diff in the TUI
ipatool tui

# 3. When happy, ACK via the TUI or CLI
ipatool pr-ack 8309 -c "LGTM"

# 4. Push to upstream
ipatool pr-push 8309 -r abbra

# 5. Backport to older branches if needed
ipatool backport 8309 -b ipa-4-12
```

All of steps 2–5 can also be done entirely inside the TUI (`ipatool tui`).

### Offline review workflow

```bash
# 1. While online, populate the cache (repeat per profile if needed)
ipatool cache-update
ipatool --profile codeberg cache-update   # if using a Codeberg profile

# 2. Work offline — review, ACK, and comment without network
ipatool --offline tui
ipatool --offline --profile codeberg tui  # Codeberg profile, separate cache

# 3. When back online, inspect what was queued
ipatool queue-list

# 4. Replay actions against the correct forge
ipatool queue-submit
# or press 's' inside the TUI
```
