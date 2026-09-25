# Development tasks for depup. Run `make` with no arguments to list the targets.
#
# Tool versions are pinned in mise.toml. When mise is available, commands run through
# `mise exec --`, so they use the pinned versions even if mise is not activated in your
# shell (or make is started from an IDE or GUI). To use the tools on PATH instead,
# pass SYSTEM_TOOLS=1 (the versions are then not guaranteed to match CI).
#
# Written for the GNU Make 3.81 that ships with macOS
# (no .ONESHELL / .SHELLFLAGS / $(file ...) / !=).

.DEFAULT_GOAL := help

BINARY_NAME := depup
INSTALL_PATH ?= /usr/local/bin
# Cargo.lock is committed, so resolve dependencies exactly as CI does
CARGO_FLAGS ?= --locked

# ---- Toolchain ----------------------------------------------------------------
# Look for mise on PATH, then in common install locations, because make started from
# a GUI may not inherit the shell's PATH. Override with make MISE=/path/to/mise.
# To try the behavior without mise, empty the candidates with MISE_CANDIDATES=.
MISE_CANDIDATES ?= $(HOME)/.local/bin/mise /opt/homebrew/bin/mise /usr/local/bin/mise
ifeq ($(SYSTEM_TOOLS),1)
RUN :=
else
ifndef MISE
MISE := $(firstword $(shell command -v mise 2>/dev/null) $(wildcard $(MISE_CANDIDATES)))
endif
ifeq ($(MISE),)
ifneq ($(filter-out help,$(or $(MAKECMDGOALS),help)),)
$(error mise was not found. Install it from https://mise.jdx.dev, or add SYSTEM_TOOLS=1 to use the tools on PATH)
endif
endif
RUN := $(if $(MISE),$(MISE) exec --,)
endif

.PHONY: help setup build release run test test-e2e test-integration lint fmt fmt-check check ci install uninstall clean

## Setup

setup: ## Install the toolchain (mise) and dependencies
	@if [ -n "$(MISE)" ]; then "$(MISE)" install; fi
	$(RUN) cargo fetch $(CARGO_FLAGS)

## Build

build: ## Build a debug binary
	$(RUN) cargo build $(CARGO_FLAGS)

release: ## Build a release binary
	$(RUN) cargo build --release $(CARGO_FLAGS)

run: ## Run the debug binary (arguments via ARGS="...")
	$(RUN) cargo run $(CARGO_FLAGS) -- $(ARGS)

## Checks

test: ## Run the tests
	$(RUN) cargo test $(CARGO_FLAGS)

test-e2e: ## Run E2E tests only
	$(RUN) cargo test $(CARGO_FLAGS) --test e2e_tests

test-integration: ## Run integration tests only
	$(RUN) cargo test $(CARGO_FLAGS) --test integration_tests

lint: ## Run clippy with warnings as errors
	$(RUN) cargo clippy $(CARGO_FLAGS) --all-targets -- -D warnings

fmt: ## Format the code (rewrites files)
	$(RUN) cargo fmt --all

fmt-check: ## Check the formatting (no changes)
	$(RUN) cargo fmt --all -- --check

check: fmt-check lint ## Run fmt-check and lint (no changes)

ci: check test ## Run the same checks as CI (no changes)

## Install

# Replace the binary through a temporary file + rename instead of copying over it.
# macOS caches code signature validation per inode, so overwriting a recently run
# binary with cp gets the new one killed with SIGKILL right after launch (exit 137).
# The temporary file lives in the same directory so that the rename swaps the inode.
install: release ## Install the release binary to INSTALL_PATH (default /usr/local/bin)
	@mkdir -p "$(INSTALL_PATH)"
	cp "target/release/$(BINARY_NAME)" "$(INSTALL_PATH)/$(BINARY_NAME).new"
	mv -f "$(INSTALL_PATH)/$(BINARY_NAME).new" "$(INSTALL_PATH)/$(BINARY_NAME)"

uninstall: ## Remove the binary from INSTALL_PATH
	rm -f "$(INSTALL_PATH)/$(BINARY_NAME)"

clean: ## Remove build artifacts
	$(RUN) cargo clean

## Help

help: ## Show this help
	@echo "Development tasks for $(BINARY_NAME)"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Tool versions are pinned in mise.toml. Run make setup first."
	@echo "Release: GitHub Actions > Release > Run workflow"
