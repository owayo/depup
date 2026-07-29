.PHONY: build release install clean test fmt check help

# 既定のターゲット
.DEFAULT_GOAL := help

# 変数
BINARY_NAME := depup
INSTALL_PATH := /usr/local/bin

## ビルドコマンド

build: ## Build debug version
	cargo build

release: ## Build release version
	cargo build --release

## インストール

install: release ## Build release and install to /usr/local/bin
	cp target/release/$(BINARY_NAME) $(INSTALL_PATH)/

## 開発

test: ## Run tests
	cargo test

test-e2e: ## Run E2E tests only
	cargo test --test e2e_tests

test-integration: ## Run integration tests only
	cargo test --test integration_tests

fmt: ## Format code
	cargo fmt

check: ## Run clippy and check
	cargo clippy -- -D warnings
	cargo check

clean: ## Clean build artifacts
	cargo clean

## ヘルプ

help: ## Show this help message
	@echo "depup Build Commands"
	@echo ""
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Release:"
	@echo "  Use GitHub Actions > Release > Run workflow"
