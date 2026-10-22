# depup の開発タスク。引数なしの make でターゲット一覧を表示する。
#
# ツールの版は mise.toml で固定する。mise があれば mise exec -- 経由で実行するため、
# シェルで mise を有効にしていなくても、IDE や GUI から make を起動しても固定版を使う。
# PATH 上のツールを使う場合は SYSTEM_TOOLS=1 を渡す（CI と同じ版とは限らない）。
#
# macOS に付属する GNU Make 3.81 に対応する。
# .ONESHELL・.SHELLFLAGS・$(file ...)・!= は使わない。

.DEFAULT_GOAL := help

BINARY_NAME := depup
INSTALL_PATH ?= /usr/local/bin
# Cargo.lock を管理しているため、CI と同じ依存を解決する。
CARGO_FLAGS ?= --locked

# ---- ツールチェーン ------------------------------------------------------------
# GUI から起動した make がシェルの PATH を継承しない場合に備え、PATH の後に
# 一般的な配置先を探す。make MISE=/path/to/mise で上書きできる。
# mise がない場合の動作を試すには、MISE_CANDIDATES= で候補を空にする。
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

## 準備

setup: ## ツールチェーン（mise）と依存を取得する
	@if [ -n "$(MISE)" ]; then "$(MISE)" install; fi
	$(RUN) cargo fetch $(CARGO_FLAGS)

## ビルド

build: ## デバッグ用バイナリをビルドする
	$(RUN) cargo build $(CARGO_FLAGS)

release: ## リリース用バイナリをビルドする
	$(RUN) cargo build --release $(CARGO_FLAGS)

run: ## デバッグ用バイナリを実行する（引数は ARGS="..."）
	$(RUN) cargo run $(CARGO_FLAGS) -- $(ARGS)

## 検査

test: ## テストを実行する
	$(RUN) cargo test $(CARGO_FLAGS)

test-e2e: ## E2E テストだけを実行する
	$(RUN) cargo test $(CARGO_FLAGS) --test e2e_tests

test-integration: ## 統合テストだけを実行する
	$(RUN) cargo test $(CARGO_FLAGS) --test integration_tests

lint: ## 警告をエラーとして clippy を実行する
	$(RUN) cargo clippy $(CARGO_FLAGS) --all-targets -- -D warnings

fmt: ## コードを整形する（書き換える）
	$(RUN) cargo fmt --all

fmt-check: ## 書式を検査する（書き換えない）
	$(RUN) cargo fmt --all -- --check

check: fmt-check lint ## fmt-check と lint を実行する（書き換えない）

ci: check test ## CI と同じ検査を実行する（書き換えない）

## インストール

# 一時ファイルと rename でバイナリを置き換える。
# macOS は inode ごとにコード署名の検証をキャッシュするため、実行直後のバイナリを
# cp で上書きすると、次の起動直後に SIGKILL で終了する（exit 137）。
# 同じディレクトリに一時ファイルを置き、rename で inode を入れ替える。
install: release ## リリース用バイナリを INSTALL_PATH へ入れる（既定値 /usr/local/bin）
	@mkdir -p "$(INSTALL_PATH)"
	cp "target/release/$(BINARY_NAME)" "$(INSTALL_PATH)/$(BINARY_NAME).new"
	mv -f "$(INSTALL_PATH)/$(BINARY_NAME).new" "$(INSTALL_PATH)/$(BINARY_NAME)"

uninstall: ## INSTALL_PATH からバイナリを削除する
	rm -f "$(INSTALL_PATH)/$(BINARY_NAME)"

clean: ## ビルド生成物を削除する
	$(RUN) cargo clean

## ヘルプ

help: ## このヘルプを表示する
	@echo "Development tasks for $(BINARY_NAME)"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Tool versions are pinned in mise.toml. Run make setup first."
	@echo "Release: GitHub Actions > Release > Run workflow"
