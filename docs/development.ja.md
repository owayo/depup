# 開発

depup をソースからビルドする人と、メンテナンスする人向けの説明です。準備と標準の `make` ターゲットは、README の「[開発](../README.ja.md#開発)」にあります。

## 検査と CI

CI の quality ジョブは、Linux と macOS で `make setup` と `make ci` を実行します。手元の `make ci` も同じ検査です。整形の確認、clippy、全テストを行います。Windows では、CI の build ジョブが `cargo test --locked` を直接実行します。リリースのワークフローは dry_run では何もビルドしないため、配布する 5 つのターゲットのビルドも CI の build ジョブで確かめています。

両方の CI ジョブで `cache-bin: false` を指定し、Cargo の依存とビルド結果をキャッシュします。`~/.cargo/bin` を復元すると mise が使う rustup を上書きし、macOS で SIGKILL されることがあるため、ここは対象から外しています。

mise を使わずに `PATH` 上のツールで動かす場合は `SYSTEM_TOOLS=1` を付けてください。その場合、版が CI と一致する保証はありません。

Windows では、レシピが POSIX シェルを使うため `make` のターゲットを Git Bash で実行します。PowerShell から固定ツールチェーンで同じ検査を行う場合は、次のコマンドを使います。

```powershell
mise install
mise exec -- cargo fetch --locked
mise exec -- cargo fmt --all -- --check
mise exec -- cargo clippy --locked --all-targets -- -D warnings
mise exec -- cargo test --locked
```

## テスト用のターゲット

`make test` のほかに、テストスイートを 1 つだけ実行するターゲットがあります。

| コマンド | 説明 |
|---|---|
| `make test-e2e` | E2E テストだけを実行する |
| `make test-integration` | 統合テストだけを実行する |

## JSON マニフェストの性能計測

固定したツールチェーンで、メモリ内の更新処理を計測できます。

```bash
mise exec -- cargo run --release --locked --example manifest_update_bench -- 1000 7
```

引数は 1 サンプルの更新回数とサンプル数です。各ケースで 100 回ウォームアップしてから、更新 1 回のナノ秒と、ウォームアップ後の 1 回で要求したメモリ確保回数・バイト数を CSV に出します。時間の計測中は確保量の集計を止めます。確保バイト数には再確保の要求も含み、ピークメモリ使用量ではありません。

Node の依存 1・100・1,000 件、エスケープしたキーと CRLF、ネストした Bun catalogs、Composer の依存 1,000 件を対象に、末尾の依存を更新します。更新結果の JSON と、依存セクション外の同名キーを保持していることも検証します。ネットワークとディスクへの書き込みは含めず、マニフェストの書き換えだけを測ります。同じ入力・依存・ツールチェーン・release 設定で、ビルドやテストを並行実行せず比較してください。測定記録は [benchmarks/json-manifest](../benchmarks/json-manifest/README.md) にあります。

## Cargo マニフェストの性能計測

第 3 引数で Cargo のケースを選びます。

```bash
mise exec -- cargo run --release --locked --example manifest_update_bench -- 1000 7 cargo
```

ウォームアップ・時間計測・確保量の集計は JSON のケースと共通です。単純指定は依存 1・100・1,000 件、inline table・dotted key・複数行テーブルは各 100 件を測ります。3 つのテーブル形式では git tag の更新も測ります。入力は単一引用符・CRLF・最終行の改行なしとし、出力全体の一致と TOML の妥当性を確認します。測定記録と対象範囲は [benchmarks/cargo-manifest](../benchmarks/cargo-manifest/README.ja.md) にあります。

## crates.io の取得の性能計測

crates.io から版の一覧を取る時間を含め、CLI 全体の所要時間を測れます。結果はネットワークとレジストリの状態に左右されるため、変更前と変更後の release ビルドを交互に実行して比べます。

```bash
bash benchmarks/crates-io-fetch/measure.sh before ./depup-before 3 > before-a.csv
bash benchmarks/crates-io-fetch/measure.sh after ./depup-after 3 > after-a.csv
bash benchmarks/crates-io-fetch/measure.sh after ./depup-after 3 > after-b.csv
bash benchmarks/crates-io-fetch/measure.sh before ./depup-before 3 > before-b.csv
```

引数は実装名、depup の実行ファイル、入力ごとの実行回数です。入力は depup 自身の `Cargo.toml`（直接依存 22 件、既定の設定）と、依存 60 件の合成マニフェスト（`--no-osv`）で、どちらも `--dry-run` で実行します。Unix では `HOME` を一時ディレクトリに向け、利用者のグローバル設定を読ませません。測定記録は [benchmarks/crates-io-fetch](../benchmarks/crates-io-fetch/README.ja.md) にあります。

npm と PyPI の取得は `benchmarks/http-compression/measure.sh` で同じように測ります。引数も実行の順序も同じです。入力は npm と PyPI のよく使われるパッケージ各 30 件の合成マニフェストで、`--dry-run --no-osv` で実行します。HTTP の圧縮 (gzip) を有効にしたときの測定記録は [benchmarks/http-compression](../benchmarks/http-compression/README.ja.md) にあります。

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
