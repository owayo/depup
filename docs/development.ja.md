# 開発

depup をソースからビルドする人と、メンテナンスする人向けの説明です。準備と標準の `make` ターゲットは、README の「[開発](../README.ja.md#開発)」にあります。

## 検査と CI

CI の quality ジョブは、Linux と macOS で `make setup` と `make ci` を実行します。手元の `make ci` も同じ検査です。整形の確認、clippy、全テストを行います。Windows では、CI の build ジョブが `cargo test --locked` を直接実行します。リリースのワークフローは dry_run では何もビルドしないため、配布する 5 つのターゲットのビルドも CI の build ジョブで確かめています。

mise を使わずに `PATH` 上のツールで動かす場合は `SYSTEM_TOOLS=1` を付けてください。その場合、版が CI と一致する保証はありません。

## テスト用のターゲット

`make test` のほかに、テストスイートを 1 つだけ実行するターゲットがあります。

| コマンド | 説明 |
|---|---|
| `make test-e2e` | E2E テストだけを実行する |
| `make test-integration` | 統合テストだけを実行する |

## リリース

リリースは GitHub Actions から行います。**Actions > Release > Run workflow** を開いて実行してください。

最初は **dry_run** にチェックを入れて試せます。dry_run では次の版を計算して表示するだけで、コミット、タグ付け、ビルド、GitHub Release の作成、Homebrew tap の更新、winget への提出はどれも行いません。

版は `yy.m.counter` の形式です（例: `26.9.100`）。counter は毎月 100 から始まり、その月のリリースごとに 1 ずつ増えます。月の区切りは日本時間で判定します。

dry_run を付けずに実行すると、次の順に処理します。

1. `Cargo.toml` の版を上げ、`Cargo.lock` は depup 自身の項目だけを同期します（`cargo update --workspace`。依存は解決し直しません）。変更をコミットし、`v<版>` のタグを付けて両方を push します。同じタグがすでにあれば、ここで止まります
2. 5 つのターゲット（Linux x86_64 / ARM64、macOS Apple Silicon / Intel、Windows x86_64）をビルドし、アーカイブと `SHA256SUMS` を添えて GitHub Release を作成します
3. tap 用の GitHub App が設定されていれば、Homebrew tap（[owayo/homebrew-depup](https://github.com/owayo/homebrew-depup)）を更新します
4. winget のトークンが設定され、`owayo.depup` が winget-pkgs に登録済みであれば、新しい版を winget-pkgs に提出します。条件を満たさないときは、この手順を飛ばします
5. 各ジョブの結果を、実行のサマリーに書き出します

版を上げたあとのジョブが失敗したときは、同じ実行の **Re-run failed jobs** でやり直します。**Re-run all jobs** を使うと版の計算からやり直すことになり、タグがすでにあるため途中で止まります。
