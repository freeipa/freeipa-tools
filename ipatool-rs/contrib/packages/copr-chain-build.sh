#!/bin/bash
# copr-chain-build.sh — submit the ipatool SRPM to COPR.
#
# ipatool is a single-package workspace; there is no build dependency chain
# between multiple packages.
#
# Usage:
#   copr-chain-build.sh [OPTIONS] COPR_PROJECT
#
# Options:
#   -d, --srpm-dir DIR     Directory containing *.src.rpm files
#                          (default: directory of this script)
#   -r, --chroot CHROOT    Add a chroot to build in (may be repeated;
#                          default: whatever the project has enabled)
#   -n, --dry-run          Print copr-cli commands without running them
#   -w, --wait             Wait for the build to finish and report status
#                          (default: submit and exit)
#   -h, --help             Show this help
#
# Examples:
#   copr-chain-build.sh @mygroup/ipatool
#   copr-chain-build.sh --chroot fedora-42-x86_64 --wait myuser/ipatool-staging
#   copr-chain-build.sh --srpm-dir /tmp/srpms --dry-run @mygroup/ipatool

set -euo pipefail

# ── helpers ───────────────────────────────────────────────────────────────────

die()  { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
info() { echo "[$(date '+%H:%M:%S')] $*"; }

usage() {
    sed -n '/^# Usage:/,/^[^#]/{ /^[^#]/q; s/^# \{0,2\}//; p }' "$0"
    exit 0
}

# Extract a single build ID from copr-cli --nowait output.
# copr-cli prints: "Created builds: 12345678"
extract_build_id() {
    local output="$1"
    local id
    id=$(echo "$output" | grep -oE 'Created builds: [0-9]+' | grep -oE '[0-9]+$' | tail -1)
    [[ -n "$id" ]] || die "Could not parse build ID from copr-cli output:\n$output"
    echo "$id"
}

# Find the latest (by version sort) SRPM matching a package name prefix.
find_srpm() {
    local name="$1" dir="$2"
    local found
    # shellcheck disable=SC2012
    found=$(ls -1 "$dir"/"$name"-[0-9]*.src.rpm 2>/dev/null | sort -V | tail -1)
    echo "$found"
}

# ── argument parsing ──────────────────────────────────────────────────────────

SRPM_DIR="$(cd "$(dirname "$0")" && pwd)"
CHROOT_ARGS=()
DRY_RUN=0
WAIT=0
COPR_PROJECT=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        -d|--srpm-dir)  [[ $# -ge 2 ]] || die "--srpm-dir requires an argument"
                        SRPM_DIR="$2"; shift 2 ;;
        -r|--chroot)    [[ $# -ge 2 ]] || die "--chroot requires an argument"
                        CHROOT_ARGS+=(-r "$2"); shift 2 ;;
        -n|--dry-run)   DRY_RUN=1; shift ;;
        -w|--wait)      WAIT=1; shift ;;
        -h|--help)      usage ;;
        -*)             die "Unknown option: $1" ;;
        *)              COPR_PROJECT="$1"; shift ;;
    esac
done

[[ -n "$COPR_PROJECT" ]] || die "COPR_PROJECT is required.  Run with --help for usage."
[[ -d "$SRPM_DIR" ]]     || die "SRPM directory not found: $SRPM_DIR"

# ── locate SRPM ───────────────────────────────────────────────────────────────

SRPM=$(find_srpm "ipatool" "$SRPM_DIR")
[[ -n "$SRPM" ]] || die "No ipatool-*.src.rpm found in $SRPM_DIR"
info "Found: $(basename "$SRPM")"
echo ""

# ── submit ────────────────────────────────────────────────────────────────────

cmd=(copr-cli build --nowait)
[[ ${#CHROOT_ARGS[@]} -gt 0 ]] && cmd+=("${CHROOT_ARGS[@]}")
cmd+=("$COPR_PROJECT" "$SRPM")

info "Submitting ipatool to $COPR_PROJECT"
info "  $(basename "$SRPM")"

if [[ "$DRY_RUN" == 1 ]]; then
    info "[dry-run] ${cmd[*]}"
    BUILD_ID="9999999"
else
    out=$("${cmd[@]}" 2>&1) || die "copr-cli failed:\n$out"
    echo "$out" | grep -v '^$' | sed 's/^/  /'
    BUILD_ID=$(extract_build_id "$out")
fi

echo ""
info "Build ID: $BUILD_ID"
info "Monitor at: https://copr.fedorainfracloud.org/coprs/${COPR_PROJECT}/builds/"
echo ""

# ── optional: wait for completion ────────────────────────────────────────────

if [[ "$WAIT" == 1 ]]; then
    info "Waiting for build $BUILD_ID to finish..."
    if [[ "$DRY_RUN" == 1 ]]; then
        info "[dry-run] would run: copr-cli watch-build $BUILD_ID"
    else
        if copr-cli watch-build "$BUILD_ID"; then
            info "Build SUCCEEDED."
        else
            die "Build FAILED.  Check the COPR web UI."
        fi
    fi
fi
