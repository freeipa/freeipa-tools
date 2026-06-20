#!/bin/bash
# check-crate-deps — query dnf repos for crates listed in a Cargo.lock
#
# Reads a Cargo.lock, extracts every crates.io registry dependency, then
# checks whether the matching crate() RPM provide is present in the currently
# configured dnf repositories.  Git and path (workspace) dependencies are
# skipped — they have no system RPM equivalent.
#
# Usage:
#   ./check-crate-deps.sh [OPTIONS] [Cargo.lock]
#   ./check-crate-deps.sh path/to/Cargo.lock
#   ./check-crate-deps.sh --align path/to/Cargo.lock
#   ./check-crate-deps.sh --align --apply path/to/Cargo.lock
#
# Options:
#   --align       print 'cargo update --precise' commands that would bring
#                 Cargo.lock in line with the versions available in Fedora
#                 (dry-run; no files are modified)
#   --apply       execute the alignment commands, patching Cargo.toml files
#                 when 'cargo update --precise' alone is insufficient
#                 (implies --align)
#   --help, -h    show this help
#
# Exit codes:
#   0  — all registry crates are available as system packages (exact version)
#   1  — one or more crates are completely absent from Fedora
#   2  — bad arguments / file not found

set -euo pipefail

# ── Argument parsing ──────────────────────────────────────────────────────────

ALIGN=0
APPLY=0
LOCKFILE=""

usage() {
    sed -n '/^# Usage:/,/^[^#]/{ /^[^#]/q; s/^# \{0,2\}//; p }' "$0"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --align|-a) ALIGN=1; shift ;;
        --apply)    APPLY=1; ALIGN=1; shift ;;
        --help|-h)  usage; exit 0 ;;
        --)         shift; break ;;
        -*)         printf 'error: unknown option: %s\n' "$1" >&2; exit 2 ;;
        *)          LOCKFILE="$1"; shift ;;
    esac
done

LOCKFILE="${LOCKFILE:-Cargo.lock}"

if [[ ! -f "$LOCKFILE" ]]; then
    printf 'error: %s: file not found\n' "$LOCKFILE" >&2
    exit 2
fi

# ── 1. Parse Cargo.lock ───────────────────────────────────────────────────────
# The file is TOML; each package is a [[package]] section.  We only care about
# crates whose source starts with "registry+" (i.e. comes from crates.io).
# Git and path deps are silently skipped.
readarray -t CRATES < <(awk '
    /^\[\[package\]\]/ {
        if (name != "" && version != "" && registry)
            print name " " version
        name = ""; version = ""; registry = 0
        next
    }
    /^name = /              { sub(/^name = "/, ""); sub(/"$/, ""); name = $0 }
    /^version = /           { sub(/^version = "/, ""); sub(/"$/, ""); version = $0 }
    /^source = "registry\+/ { registry = 1 }
    END {
        if (name != "" && version != "" && registry)
            print name " " version
    }
' "$LOCKFILE" | sort -u)

total=${#CRATES[@]}
if [[ $total -eq 0 ]]; then
    printf 'No crates.io entries found in %s.\n' "$LOCKFILE"
    exit 0
fi

# ── 2. Fetch crate() provides from all configured repos (single dnf query) ────
# dnf repoquery --provides is run once and its output filtered to crate() lines.
# This is far faster than invoking dnf once per crate.
#
# --refresh forces a repo metadata download on every invocation.  Without it,
# dnf serves results from the local metadata cache, which may lag behind the
# actual repo state by hours or days.
#
# A temp file is used rather than a shell variable because storing the result in
# a variable and then piping it through "printf '%s\n' "$var" | grep -qF ..." is
# broken under "set -o pipefail": grep -q exits as soon as it finds a match,
# which causes printf to receive SIGPIPE (exit 141).  pipefail then reports the
# pipeline as failed and the crate is incorrectly classified as missing.
# Reading directly from a file avoids that SIGPIPE path entirely.
printf 'Fetching crate() provides from dnf repos (refreshing metadata)... '
_provides_tmp=$(mktemp -t check-crate-deps.XXXXXX)
trap 'rm -f "$_provides_tmp"' EXIT
dnf repoquery --provides --quiet --refresh 2>/dev/null \
    | grep '^crate(' | sort -u > "$_provides_tmp" || true
printf 'done (%d provides).\n\n' "$(wc -l < "$_provides_tmp")"

# ── 3. Check each crate ───────────────────────────────────────────────────────
printf 'Checking %d registry crates from %s:\n\n' "$total" "$LOCKFILE"

found=0
mismatch=0
missing=0
declare -a mismatch_list=()
declare -a missing_list=()
# Machine-readable mismatch records: "name need_ver avail_ver"
declare -a mismatch_pairs=()

for entry in "${CRATES[@]}"; do
    name="${entry%% *}"
    ver="${entry##* }"
    provide="crate(${name}) = ${ver}"

    # grep -F: fixed-string match; the parentheses in "crate(NAME) = VER" make
    # accidental partial matches (e.g. crate(foo) vs crate(foobar)) impossible.
    if grep -qF "$provide" "$_provides_tmp"; then
        printf '  ok  %s\n' "$provide"
        found=$(( found + 1 ))
    else
        # Exact version not present.  Check whether the crate exists at *any*
        # version in the repos: a version mismatch (crate packaged but at a
        # different version) is very different from a crate that is absent
        # entirely and must be shipped in the vendor tarball.
        avail_ver=$(grep -F "crate(${name}) = " "$_provides_tmp" \
                    | grep -v '/' \
                    | awk '{print $NF}' | sort -V | tail -1 || true)
        if [[ -n "$avail_ver" ]]; then
            printf ' ver  %s  (repo has: %s)\n' "$provide" "$avail_ver"
            mismatch=$(( mismatch + 1 ))
            mismatch_list+=( "${name}: need ${ver}, repo has ${avail_ver}" )
            mismatch_pairs+=( "${name} ${ver} ${avail_ver}" )
        else
            printf ' ---  %s\n' "$provide"
            missing=$(( missing + 1 ))
            missing_list+=( "$provide" )
        fi
    fi
done

# ── 4. Summary ────────────────────────────────────────────────────────────────
printf '\n%d / %d crates at exact version; %d version mismatch; %d absent.\n' \
    "$found" "$total" "$mismatch" "$missing"

if [[ ${#mismatch_list[@]} -gt 0 ]]; then
    printf '\nVersion mismatch (packaged in Fedora, but at a different version):\n'
    printf '  %s\n' "${mismatch_list[@]}"
fi

if [[ ${#missing_list[@]} -gt 0 ]]; then
    printf '\nNot available (require vendor tarball or new packaging):\n'
    printf '  %s\n' "${missing_list[@]}"
fi

# ── 5. Align mode ─────────────────────────────────────────────────────────────
# When --align is given, attempt to bring Cargo.lock in line with the Fedora
# repo versions.  The strategy per mismatched crate is:
#
#   a) Run 'cargo update --precise <fedora-ver> <crate>'.
#      This succeeds whenever the Cargo.toml constraint already permits the
#      Fedora version (e.g. "libc = "0.2"" allows any 0.2.x).
#
#   b) If cargo update rejects the version (constraint too strict), patch the
#      Cargo.toml files in the workspace to relax the minimum version to
#      major.minor of the Fedora version, then retry cargo update.

# _patch_cargo_toml NAME NEED_VER AVAIL_VER REPOROOT
_patch_cargo_toml() {
    local crate_name="$1"
    local need_ver="$2"
    local avail_ver="$3"
    local reporoot="$4"

    local -a _avail_parts
    IFS='.' read -r -a _avail_parts <<< "$avail_ver"
    local relaxed="${_avail_parts[0]}.${_avail_parts[1]}"
    local need_ver_re="${need_ver//./\\.}"
    # crate names on crates.io only contain [a-zA-Z0-9_-], safe for regex
    local crate_re="${crate_name//[-_]/[-_]}"
    local changed=0

    while IFS= read -r toml; do
        local before
        before=$(md5sum "$toml")

        sed -i -E \
            "s|^([[:space:]]*${crate_re}[[:space:]]*=[[:space:]]*\")${need_ver_re}(\"[[:space:]]*(#.*)?)$|\1${relaxed}\2|" \
            "$toml"
        sed -i -E \
            "s|^([[:space:]]*${crate_re}[[:space:]]*=.*[[:space:]]version[[:space:]]*=[[:space:]]*\")${need_ver_re}(\")|\1${relaxed}\2|" \
            "$toml"

        local after
        after=$(md5sum "$toml")
        if [[ "$before" != "$after" ]]; then
            printf '    patched %s\n' "$toml"
            changed=$(( changed + 1 ))
        fi
    done < <(find "$reporoot" -name "Cargo.toml" \
                 -not -path "*/vendor/*" \
                 -not -path "*/.git/*" \
                 -not -path "*/target/*")

    [[ "$changed" -gt 0 ]]
}

if [[ "$ALIGN" -eq 1 ]] && [[ "${#mismatch_pairs[@]}" -gt 0 ]]; then
    REPOROOT="$(dirname "$(realpath "$LOCKFILE")")"

    if [[ "$APPLY" -eq 0 ]]; then
        printf '\n── Suggested commands to align Cargo.lock with Fedora ──\n'
        printf '# Run from: %s\n\n' "$REPOROOT"
        for pair in "${mismatch_pairs[@]}"; do
            crate_name="${pair%% *}"
            avail_ver="${pair##* }"
            printf 'cargo update --precise %s %s\n' "$avail_ver" "$crate_name"
        done
        printf '\n# If a command fails, the Cargo.toml constraint is too strict.\n'
        printf '# Re-run this script with --align --apply to patch automatically.\n'
    else
        printf '\n── Aligning Cargo.lock with Fedora repo versions ──\n\n'
        align_ok=0
        align_toml=0
        align_failed=0

        for pair in "${mismatch_pairs[@]}"; do
            crate_name="${pair%% *}"
            rest="${pair#* }"
            need_ver="${rest%% *}"
            avail_ver="${rest##* }"

            printf '  %s  %s → %s\n' "$crate_name" "$need_ver" "$avail_ver"

            if ( cd "$REPOROOT" && \
                 cargo update --precise "$avail_ver" "$crate_name" 2>/dev/null )
            then
                printf '    cargo update ok\n'
                align_ok=$(( align_ok + 1 ))
                continue
            fi

            printf '    cargo update --precise rejected; patching Cargo.toml\n'
            if _patch_cargo_toml \
                   "$crate_name" "$need_ver" "$avail_ver" "$REPOROOT"
            then
                if ( cd "$REPOROOT" && \
                     cargo update --precise "$avail_ver" "$crate_name" 2>/dev/null )
                then
                    printf '    cargo update ok after Cargo.toml patch\n'
                    align_toml=$(( align_toml + 1 ))
                else
                    printf '    cargo update still failed after patching — manual intervention required\n'
                    align_failed=$(( align_failed + 1 ))
                fi
            else
                printf '    no matching version constraint found in Cargo.toml\n'
                printf '    (multi-line [dependencies.%s] tables are not auto-patched)\n' \
                       "$crate_name"
                align_failed=$(( align_failed + 1 ))
            fi
        done

        printf '\nAlign summary: %d updated via cargo update' "$align_ok"
        [[ "$align_toml" -gt 0 ]] && printf ', %d needed Cargo.toml patch' "$align_toml"
        [[ "$align_failed" -gt 0 ]] && printf ', %d could not be aligned' "$align_failed"
        printf '.\n'

        if [[ "$align_failed" -gt 0 ]]; then
            printf '\nInspect the failures above and align those crates manually.\n'
        fi
    fi
fi

# Exit codes follow the original convention: non-zero only when crates are
# completely absent from Fedora (version mismatches are advisory).
if [[ "${#missing_list[@]}" -gt 0 ]]; then
    exit 1
fi
