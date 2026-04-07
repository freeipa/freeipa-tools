# ipatool

A Rust port of the FreeIPA development workflow tool.  It automates the
repetitive parts of reviewing and landing patches: fetching patches, applying
them, pushing upstream, updating issue trackers (Pagure, Forgejo, Jira), and
managing GitHub pull requests.

## Building

```
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
# issues; update custom fields; update milestone.
pagure-token: "0123456789abcdef0123456789abcdef01234567"
# Pagure does not expose user info via token, so set it explicitly.
username: your-pagure-login

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
```

## Global flags

These flags apply to every subcommand.

| Flag | Default | Description |
|------|---------|-------------|
| `--config <path>` | `~/.ipa/toolconf.yaml` | Configuration file |
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
```

**Arguments:** `<pr_id>` (required)

**Flags**

| Flag | Description |
|------|-------------|
| `-c`, `--comment <text>` | Comment text (optional) |

---

### `pr-reject`

Add the `rejected` label, remove `ack` if present, post a mandatory comment,
and close the PR.

```
ipatool pr-reject 8309 -c "Needs rebase on top of master"
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

#### Layout

```
 a:ACK  x:Reject  c:Review  b:Browser  r:Refresh  q:Quit  …  Enter:Actions
┌── 47 PRs ────────────────────┐┌── Details ──────────────────────────────────┐
│ #8309 ○ Fix LDAP timeout  …  ││ PR #8309: Fix LDAP connection timeout        │
│ #8308 ○ Add KDC support   …  ││                                              │
│ #8307 ✓ Refactor tests [ack] ││ URL:    https://github.com/…/pull/8309       │
│ #8306 ✗ Old fix [rejected]   ││ State:  open  |  Author: @contributor        │
│ …                            ││ Labels: needs-rebase                         │
│                              ││ Base:   master                               │
│                              ││                                              │
│                              ││ CI Status:                                   │
│                              ││   ✓ ci/freeipa                               │
│                              ││   ✗ ci/lint                                  │
│                              ││                                              │
│                              ││ Changed files (3 files, +42 / -7):           │
│                              ││   M  ipaserver/plugins/ldap2.py              │
└──────────────────────────────┘└─────────────────────────────────────────────┘
```

The left pane colour-codes PRs:  green = acked, red = rejected/closed,
default = pending.  The right pane refreshes in the background as you move
between PRs.

#### Global keyboard shortcuts

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down in list |
| `k` / `↑` | Move up in list |
| `Enter` | Open action menu for selected PR |
| `a` | ACK selected PR |
| `x` | Reject selected PR |
| `c` | Open diff review for selected PR |
| `b` | Open PR in browser |
| `r` | Refresh PR list |
| `s` | Sync queued offline actions |
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
Apply / Cancel / `Esc` close the editor.  In offline mode, changes are queued.

#### Push form (`p`)

Collects reviewer(s), optional backport branches, and the autobackport flag,
then quits the TUI, runs `pr-push` in the terminal (with full output), and
returns you to the TUI afterwards.

#### Backport form (`B`)

Collects target branches (comma-separated), then quits the TUI, runs the
backport pipeline in the terminal, and returns.

---

### `cache-update`

Fetch pull requests from GitHub and store them — together with their CI
statuses, recent comments, and changed-file lists — in the local SQLite cache.
Subsequent calls are incremental: only PRs whose `updated_at` timestamp has
changed since the last fetch have their details re-fetched.

```
ipatool cache-update              # cache open PRs
ipatool cache-update --state all  # cache all PRs (open + closed)
```

**Flags**

| Flag | Default | Description |
|------|---------|-------------|
| `--state` | `open` | Which PRs to cache: `open`, `closed`, `all` |

Typical output:

```
Fetching open pull requests from GitHub…
Cached 52 pull request(s).
Fetching details for PR #8309 (1/52)…
Details: 3 updated, 49 unchanged (skipped).
```

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
- loads the PR list and full detail pane (CI status, comments, files) from the
  local SQLite database,
- queues mutations (ACK, reject, label changes, comments) in the database
  instead of executing them immediately,
- shows `[OFFLINE | N queued | s:Sync]` in the help bar.

Details for PRs viewed online are cached automatically, so revisiting them
offline shows the full information.

### Syncing queued actions

Press `s` in the TUI (or go back online) to replay the queue:

```
[OFFLINE | 3 queued | s:Sync]
```

Actions are replayed in order; the first failure halts the queue so ordering
is preserved.  Successfully replayed actions are deleted from the queue.

### What is cached

| Data | Cached when |
|------|------------|
| PR list (all fields) | `cache-update` or online TUI load |
| CI statuses, comments, changed files | `cache-update` or when a PR is selected in the online TUI |
| Mutations (ACK, reject, labels, comments) | Immediately, when performed in offline mode |

### Queued action types

| Action | Triggered by |
|--------|-------------|
| `Ack` | ACK dialog in offline TUI |
| `Reject` | Reject dialog in offline TUI |
| `UpdateLabels` | Label editor Apply in offline TUI |
| `PostComment` | General comment (future) |
| `PostReviewComment` | Comment form in offline review view |

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
field of each Pagure/Forgejo issue.  After a push, ipatool optionally:

- posts a commit-info comment (`update-jira`),
- transitions the issue (`close-jira`, using the `jira-close-transition` name).

The Jira REST API server is derived automatically from the scheme+host of
`jira-ticket-url`.  Disable Jira with `--no-jira`.

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
