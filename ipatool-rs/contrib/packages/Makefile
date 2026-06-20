# Makefile — helpers for preparing Fedora package sources and building SRPMs.
#
# Quick start:
#   make sources   # create all source files (tarball + vendor; requires network)
#   make spec      # validate rust2rpm.toml and generate ipatool.spec
#   make srpm      # build an SRPM in the current directory (no ~/rpmbuild needed)
#   make mock      # rebuild SRPM in a clean mock chroot
#
# Override GIT_REF to build from a tag:
#   make sources GIT_REF=v0.1.0
#
# Requires: git, cargo, rust2rpm, rpmbuild, rpmlint
# For 'mock': mock package + your user in the 'mock' group
#   sudo usermod -aG mock $$USER   (then log out and back in)
# For 'copr-build': copr-cli configured with your API token
#
# rust2rpm is used to validate rust2rpm.toml (which is the canonical record of
# all package metadata and build requirements) before each spec/SRPM build.
# Full spec generation via rust2rpm is not possible for workspace crates that
# are not published on crates.io — rust2rpm requires a .crate archive and
# rejects workspace Cargo.toml files.  The spec template (ipatool.spec.in) is
# therefore maintained alongside rust2rpm.toml; both must be kept in sync.

# Enable pipefail so that a failed 'git archive | gzip' pipe is not silently
# swallowed (gzip always exits 0, which would make Make think the tarball
# target succeeded and leave an empty .tar.gz behind).
SHELL       := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c

# Delete the target file when a recipe exits with a non-zero status.
.DELETE_ON_ERROR:

# ──────────────────────────────────────────────────────────────────────────────
# Variables
# ──────────────────────────────────────────────────────────────────────────────

SPEC_IN  := ipatool.spec.in
SPEC_OUT := ipatool.spec

# Absolute path to the git repository root (freeipa-tools/).
# The Makefile lives at ipatool-rs/contrib/packages/, so three levels up.
REPOROOT := $(abspath $(dir $(abspath $(lastword $(MAKEFILE_LIST))))/../../..)

# Absolute path to the ipatool-rs crate directory.
CRATEDIR := $(abspath $(dir $(abspath $(lastword $(MAKEFILE_LIST))))/../..)

# The subdirectory name inside the git repo (used for git archive filtering).
SUBDIR   := $(notdir $(CRATEDIR))

# Derive NAME and VERSION from the canonical sources (Cargo.toml)
# rather than from the spec template so that they stay correct even if the
# spec.in header is out of date.
NAME    := ipatool
VERSION := $(shell python3 -c \
    "import tomllib; print(tomllib.load(open('$(CRATEDIR)/Cargo.toml','rb'))['package']['version'])" \
    2>/dev/null)

TARBALL        := $(NAME)-$(VERSION).tar.gz
VENDOR_TARBALL := $(NAME)-$(VERSION)-vendor.tar.gz

# Absolute path to the crate dependency checker script.
CHECKDEPS := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))check-crate-deps.sh

ALL_SOURCES := $(TARBALL) $(VENDOR_TARBALL)

# Git ref used for the tarball and snapshot release identifiers.
# Defaults to HEAD so snapshots reflect the current branch tip.
# Override to a release tag for stable builds:
#   make sources GIT_REF=v0.1.0
GIT_REF ?= HEAD

# Snapshot release identifiers derived from GIT_REF.
# SNAPDATE   — commit date+time of GIT_REF in YYYYMMDDhhmm format.
# SNAPCOMMIT — abbreviated commit hash of GIT_REF (7 hex digits).
# RELEASE    — full RPM Release value (without %{?dist}).
SNAPDATE   := $(shell git -C "$(REPOROOT)" log -1 --format=%cd --date=format:%Y%m%d%H%M "$(GIT_REF)" 2>/dev/null)
SNAPCOMMIT := $(shell git -C "$(REPOROOT)" rev-parse --short "$(GIT_REF)" 2>/dev/null)
RELEASE    := 1.$(SNAPDATE).git$(SNAPCOMMIT)

# Mock chroot configuration for 'make mock'.
# Override on the command line:  make mock MOCK_CONFIG=fedora-42-x86_64
MOCK_CONFIG ?= fedora-rawhide-x86_64

# ──────────────────────────────────────────────────────────────────────────────

.PHONY: all sources tarball vendor spec srpm mock lint check-deps clean help copr-build

all: sources

help:
	@echo "Targets:"
	@echo "  sources       prepare all source files (tarball + vendor)"
	@echo "  tarball       create $(TARBALL) from GIT_REF=$(GIT_REF)"
	@echo "  vendor        create $(VENDOR_TARBALL)  [needs network]"
	@echo "  spec          validate rust2rpm.toml and generate $(SPEC_OUT)"
	@echo "  srpm          build SRPM (Release: $(RELEASE)%{?dist})"
	@echo "  mock          rebuild SRPM in mock chroot (MOCK_CONFIG=$(MOCK_CONFIG))"
	@echo "  lint          run rpmlint on the generated $(SPEC_OUT)"
	@echo "  check-deps    check which Cargo.lock crates are available as system packages"
	@echo "  copr-build    submit SRPM to COPR  (COPR=project required)"
	@echo "                example: make copr-build COPR=@mygroup/ipatool"
	@echo "  clean         remove generated source files and the generated spec"
	@echo ""
	@echo "Snapshot: GIT_REF=$(GIT_REF)  date=$(SNAPDATE)  commit=$(SNAPCOMMIT)"
	@echo ""
	@echo "Note: rust2rpm validates rust2rpm.toml before every spec/SRPM build but"
	@echo "  cannot generate the spec for this workspace (ipatool is not on crates.io;"
	@echo "  rust2rpm rejects workspace Cargo.toml files).  $(SPEC_IN) is the"
	@echo "  spec template; rust2rpm.toml is the canonical metadata record.  Keep"
	@echo "  both in sync when adding subpackages, BuildRequires, or scriptlets."

# ══════════════════════════════════════════════════════════════════════════════
# SOURCE PREPARATION
# ══════════════════════════════════════════════════════════════════════════════

sources: tarball vendor

# ── tarball ───────────────────────────────────────────────────────────────────
#
# The ipatool-rs/ subdirectory lives inside the freeipa-tools git repository.
# We use 'git archive' to extract only the ipatool-rs/ subtree, then re-pack
# it with the correct top-level directory name (ipatool-{version}/).

tarball: $(TARBALL)

$(TARBALL):
	@echo "==> Creating $@ from $(GIT_REF) (subtree: $(SUBDIR)/)"
	git -C "$(REPOROOT)" archive \
	    --prefix=$(NAME)-$(VERSION)/ \
	    "$(GIT_REF)" -- $(SUBDIR)/ \
	| gzip -9 > $@.tmp
	@# Re-pack: strip the ipatool-rs/ path component so that the tarball
	@# contains ipatool-{version}/Cargo.toml rather than
	@# ipatool-{version}/ipatool-rs/Cargo.toml.
	mkdir -p $@.repack
	tar -xzf $@.tmp -C $@.repack
	mv "$@.repack/$(NAME)-$(VERSION)/$(SUBDIR)"/* \
	   "$@.repack/$(NAME)-$(VERSION)/$(SUBDIR)"/.??* \
	   "$@.repack/$(NAME)-$(VERSION)/" 2>/dev/null || true
	rmdir "$@.repack/$(NAME)-$(VERSION)/$(SUBDIR)"
	tar -czf $@ -C $@.repack $(NAME)-$(VERSION)/
	rm -rf $@.tmp $@.repack
	@echo "==> $@ ready"

# ── vendor ────────────────────────────────────────────────────────────────────
#
# Vendor all crate dependencies with --versioned-dirs so each crate directory
# carries its version (e.g. vendor/clap-4.5.0/ rather than vendor/clap/).

vendor: $(VENDOR_TARBALL)

$(VENDOR_TARBALL):
	@echo "==> Creating $@ — vendoring dependencies"
	cd "$(CRATEDIR)" && cargo vendor --versioned-dirs vendor >/dev/null
	@# Remove .profraw coverage artefacts from vendored crate sources.
	@# Must delete the files AND scrub their entries from .cargo-checksum.json;
	@# cargo verifies all listed files during build and fails if any are missing.
	@find "$(CRATEDIR)/vendor" -name '*.profraw' -delete
	@find "$(CRATEDIR)/vendor" -name '.cargo-checksum.json' \
	    -exec python3 -c "import json,sys;[open(p,'w').write(json.dumps(dict(d,files={k:v for k,v in d['files'].items() if not k.endswith('.profraw')}),separators=(',',':'))) for p in sys.argv[1:] for d in [json.load(open(p))]]" {} +
	@# Archive vendor/ and Cargo.lock (pinned versions for reproducible builds).
	tar -czf "$(CURDIR)/$@" \
	    -C "$(CRATEDIR)" vendor/ Cargo.lock
	rm -rf "$(CRATEDIR)/vendor"
	@echo "==> $@ ready"

# ══════════════════════════════════════════════════════════════════════════════
# SPEC / SRPM / MOCK / LINT
# ══════════════════════════════════════════════════════════════════════════════

# ── spec ──────────────────────────────────────────────────────────────────────
#
# Validate rust2rpm.toml (the canonical metadata record), then generate the
# RPM spec by substituting snapshot release globals into the spec template.
#
# rust2rpm validates that all keys in rust2rpm.toml are recognised and that
# their values have the correct types.  This catches drift between the TOML
# config and the spec template before the SRPM is assembled.
#
# Full spec generation via rust2rpm is not possible for this workspace:
#   - rust2rpm requires a .crate archive as input.
#   - It explicitly rejects workspace Cargo.toml files.
#   - ipatool is not published on crates.io.
# The spec template ($(SPEC_IN)) is therefore maintained alongside
# rust2rpm.toml.  Both files must be kept in sync.

spec:
	@echo "==> Validating rust2rpm.toml"
	rust2rpm --validate-only -C "$(CURDIR)/rust2rpm.toml"
	@echo "==> Generating $(SPEC_OUT) from $(SPEC_IN)"
	sed \
	    -e "s/^%global snapdate[[:space:]].*/%global snapdate    $(SNAPDATE)/" \
	    -e "s/^%global snapcommit[[:space:]].*/%global snapcommit  $(SNAPCOMMIT)/" \
	    "$(SPEC_IN)" > "$(SPEC_OUT)"

# ── srpm ──────────────────────────────────────────────────────────────────────

srpm: sources spec
	@echo "==> Building SRPM ($(NAME)-$(VERSION)-$(RELEASE))"
	rpmbuild -bs \
	    --define "_sourcedir $(CURDIR)" \
	    --define "_specdir   $(CURDIR)" \
	    --define "_srcrpmdir $(CURDIR)" \
	    "$(SPEC_OUT)"
	rm -f "$(SPEC_OUT)"

# ── mock ──────────────────────────────────────────────────────────────────────

mock: srpm
	@SRPM=$$(ls -1t "$(CURDIR)"/$(NAME)-$(VERSION)-*.src.rpm 2>/dev/null | head -1); \
	if [ -z "$$SRPM" ]; then \
	    echo "ERROR: no SRPM found in $(CURDIR) for $(NAME)-$(VERSION)" >&2; \
	    exit 1; \
	fi; \
	echo "==> Rebuilding $$SRPM in mock ($(MOCK_CONFIG))"; \
	mock -r "$(MOCK_CONFIG)" --rebuild "$$SRPM"

# ── lint ──────────────────────────────────────────────────────────────────────
#
# Run rpmlint on the generated spec (not the .spec.in template, which contains
# unresolved substitution placeholders that rpmlint would flag as errors).

lint: spec
	rpmlint "$(SPEC_OUT)"
	rm -f "$(SPEC_OUT)"

# ══════════════════════════════════════════════════════════════════════════════
# DEPENDENCY CHECKING
# ══════════════════════════════════════════════════════════════════════════════

# Advisory check — reports which Cargo.lock crates are available as system RPMs
# and which must be satisfied by the vendor tarball.  Exit code 1 (some missing)
# is suppressed because missing crates are expected (they are vendored); only
# unexpected errors (exit code 2+) propagate.

check-deps:
	@rc=0; bash "$(CHECKDEPS)" "$(CRATEDIR)/Cargo.lock" || rc=$$?; \
	exit $$((rc > 1 ? rc : 0))

# ══════════════════════════════════════════════════════════════════════════════
# COPR SUBMISSION
# ══════════════════════════════════════════════════════════════════════════════
#
# Submit the latest SRPM to a COPR project.
#
# Usage:
#   make copr-build COPR=@mygroup/ipatool
#   make copr-build COPR=myuser/ipatool-staging COPR_OPTS="--chroot fedora-42-x86_64 --wait"

COPR      ?=
COPR_OPTS ?=

copr-build:
	@test -n "$(COPR)" || \
	    { echo "ERROR: set COPR=<project>  e.g. make copr-build COPR=@mygroup/ipatool" >&2; exit 1; }
	bash "$(dir $(abspath $(lastword $(MAKEFILE_LIST))))copr-chain-build.sh" \
	    --srpm-dir "$(CURDIR)" \
	    $(COPR_OPTS) \
	    "$(COPR)"

# ══════════════════════════════════════════════════════════════════════════════
# CLEAN
# ══════════════════════════════════════════════════════════════════════════════

clean:
	rm -f $(ALL_SOURCES) "$(SPEC_OUT)"
	rm -rf "$(CRATEDIR)/vendor"
