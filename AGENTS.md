# depup

Multi-language dependency updater CLI tool

## Project Overview

depup は複数のプログラミング言語のパッケージ依存関係を一括で最新バージョンに更新するCLIツール。各言語のレジストリAPIからバージョン情報を取得し、マニフェストファイルを直接更新する。

## Tech Stack

- **Language**: Rust (Edition 2024)
- **Async Runtime**: tokio (full features)
- **HTTP Client**: reqwest (json feature)
- **CLI Framework**: clap (derive feature)
- **Serialization**: serde / serde_json / toml
- **Error Handling**: thiserror / anyhow
- **Date/Time**: chrono

## Architecture

```
src/
  main.rs          - CLI エントリポイント
  lib.rs           - ライブラリエクスポート
  cli.rs           - CLI引数定義 (clap derive)
  config.rs        - .depup 設定ファイルパーサ (モノレポ対応)
  orchestrator.rs  - ワークフロー制御 (detect → parse → fetch → judge → write)
  error.rs         - エラー型定義 (thiserror)
  progress.rs      - プログレスバー表示
  package_manager.rs - パッケージマネージャinstall連携
  tauri_sync.rs    - Tauriバージョン同期
  domain/
    language.rs      - 対応言語enum (Node/Python/Rust/Go/Ruby/PHP/Java/Swift)
    dependency.rs    - 依存関係構造体 (Registry / Git 両対応)
    git_source.rs    - Git 依存の情報型 (GitReference / GitSource)
    version_spec.rs  - バージョン指定種別 (Caret/Tilde/Range等)
    update_result.rs - 更新判定結果
    summary.rs       - 更新サマリ
  manifest/
    detector.rs      - マニフェストファイル検出
    writer.rs        - マニフェストファイル書き込み (git tag 更新含む)
    package_json.rs  - Node.js パーサ
    pyproject_toml.rs - Python パーサ
    cargo_toml.rs    - Rust パーサ (git 依存検出対応)
    cargo_lock.rs    - Cargo.lock から git 依存の現在 commit 抽出
    go_mod.rs        - Go パーサ
    gemfile.rs       - Ruby パーサ
    composer_json.rs - PHP パーサ
    gradle.rs        - Java パーサ
    package_swift.rs - Swift パーサ
    pnpm_settings.rs - pnpm設定読み取り
  parser/           - 言語別パース処理
  registry/
    client.rs        - HTTP共通クライアント
    npm.rs           - npm Registry
    pypi.rs          - PyPI
    crates_io.rs     - crates.io
    go_proxy.rs      - Go Module Proxy
    rubygems.rs      - RubyGems
    packagist.rs     - Packagist
    maven_central.rs - Maven Central
    github_tags.rs   - GitHub Tags (Swift)
    git_remote.rs    - git ls-remote で branch/tag/HEAD を取得 (Rust git 依存向け)
  update/
    filter.rs        - フィルタ設定
    version_info.rs  - バージョン情報・比較
  output/
    text.rs          - テキスト出力
    json.rs          - JSON出力
    diff.rs          - diff出力
tests/
  integration_tests.rs - 統合テスト (マニフェスト検出・パース・パイプライン)
  e2e_tests.rs         - E2Eテスト (バイナリ実行)
```

## Key Patterns

- **ManifestParser trait**: 各言語のパーサが実装 (`parse`, `update_version`)
- **RegistryAdapter trait**: 各レジストリアダプタが実装 (`fetch_versions`)
- **UpdateJudge**: フィルタ条件に基づく更新判定エンジン
- **VersionSpec**: バージョン制約の種類保持とフォーマット保存
- **DepupConfig**: `.depup` ファイルによるモノレポ対応 (複数ディレクトリ一括処理)
- **VersionCache**: レジストリ応答のキャッシュ (同一パッケージの重複フェッチ防止)

## Supported Languages

| Language | Manifest | Registry |
|----------|----------|----------|
| Node.js | package.json | npm |
| Python | pyproject.toml | PyPI |
| Rust | Cargo.toml (workspace members 自動検出) | crates.io |
| Go | go.mod | Go Proxy |
| Ruby | Gemfile | RubyGems |
| PHP | composer.json | Packagist |
| Java | build.gradle / build.gradle.kts | Maven Central |
| Swift | Package.swift | GitHub Tags |

## Development Commands

```bash
cargo build              # デバッグビルド
cargo build --release    # リリースビルド
cargo test               # 全テスト実行
cargo test --test integration_tests  # 統合テストのみ
cargo test --test e2e_tests          # E2Eテストのみ
cargo clippy -- -D warnings         # Lint
cargo fmt                # フォーマット
make help                # Makefileヘルプ
```

## Testing Strategy

- **Unit tests**: 各モジュール内に `#[cfg(test)]` で配置
- **Integration tests**: `tests/integration_tests.rs` - マニフェスト検出、パース、パイプラインテスト
- **E2E tests**: `tests/e2e_tests.rs` - バイナリ実行テスト (dry-run、JSON出力、exit code)
- ネットワークアクセスを伴うテストは E2E テストに限定

## Important Notes

- バージョン比較は数値ベースの semver 比較を使用 (文字列比較ではない)
- プレリリースバージョン (alpha/beta/canary/dev/rc) はデフォルトでフィルタされる
- 作者が非推奨を示すためにリリース末尾へ付与するマーカー (`-deprecated` / `-obsolete` / `-retired` / `-yanked` / `-unmaintained`) も prerelease として扱われ、デフォルト更新対象から除外される (例: `serde_yaml 0.9.34-deprecated` は 0.9.33 から更新されない)
- Go は常に pinned 扱い (`--include-pinned` 不要) だが、`// pinned` コメント付き依存は `--include-pinned` がないとスキップされる
- Ruby の `group` ブロックはネストを考慮して判定され、内側の `:development` / `:test` を抜けた後の gem は開発依存として漏れない。`platforms` / `source` 等のネストされた `do...end` ブロックもグループスタックを壊さず正しく追跡する
- Gemfile の複合制約・除外制約（例: `'>= 0.18', '< 2.0'`, `'!= 2.2.4'`）は解析対象だが、安全に書き換えられないため自動更新ではエラーとして扱う
- Java/Gradle の strict 記法（例: `1.2.3!!`）は固定バージョンとして解釈され、`!!` を保持して更新される
- Node/Python/Rust/PHP/Gradle の部分ワイルドカード指定（例: `1.x`, `1.x.x`, `v1.*`, `1.2.*`, `1.+`）は形を保って更新される
- 完全浮動指定（例: `*`, npm dist-tag の `latest`, Gradle の `latest.release` / `latest.integration`）は意味を変えないため更新対象から除外される
- Range制約 (`>=X,<Y` / `>=X,<=Y` / `A..<B` / `A...B` / `A - B` / `[A,B)` / `[A,B]` / `]A,B[` / `[A,B[` / `]A,B]`) では上限を超えるバージョンは除外され、更新時は上限制約を維持したまま下限側のみを互換な最新バージョンへ進める (`<=` / `...` / 閉じ `]` は上限値を含む。npm/Composer の `A - B` は右辺が完全指定なら包含、`1.0 - 2.0` のような部分指定ならワイルドカード展開後の排他的上限として扱う)
- 安全に書き換えられない複合制約（例: npm/Composer の `^1 || ^2`、除外専用制約 `!=1.2.3`、下限なし Maven 形式 `(,2.0]`）は更新候補から除外される
- Maven 形式の qualifier 付き上限（例: `[1.0,2.0.Final)`）も上限制約として解釈される
- Cargo workspace の `[workspace] members` に指定されたメンバークレートの Cargo.toml も自動検出
- Tauriプロジェクトでは npm/crate のバージョンを自動同期
- Swift は GitHub Tags API を使用 (`GITHUB_TOKEN`/`GH_TOKEN` で認証可能)
- Swift の GitHub タグは `v1.2.3` と `V1.2.3` の両方を認識する
- Swift の非 GitHub URL はスキップされる (警告なし)
- Swift の `branch:` / `revision:` 依存はバージョンなしとしてスキップ
- Rust (Cargo) の演算子は `>= 1.2.3` のようにスペースを含む形式も対応
- Python の Range 制約は単一セグメントバージョン（例: `>=3,<4`）も正しくパースする
- Go の `replace` ディレクティブ（単一行・ブロック形式とも）はパースと更新の両方でスキップされる
- Go の `exclude` ディレクティブ（単一行・ブロック形式とも）はパースと更新の両方でスキップされる
- Maven Central のクエリはグループID/アーティファクトIDの文字種を検証し、不正な文字によるURLインジェクションを防止する
- npm/Composer は semver の prerelease (`-...`) と build metadata (`+...`) を同時に含むバージョン（例: `^1.2.3-rc.1+build123`）も正しくパースする
- Rust (Cargo) の git 依存 (`{ git = "...", branch/tag/rev = "..." }` / 省略形) を検出し、`git ls-remote` でリモート HEAD / タグを取得して更新判定する。branch / 省略形 (デフォルトブランチ) は最新コミットへ更新 (Cargo.toml は書き換えず、Cargo.lock を `--install` の `cargo update` で再解決)、tag は最新 semver タグへ更新 (Cargo.toml の tag 文字列を書き換え)、rev は常にスキップ (pinned 扱い)
- Cargo.lock の git source 末尾 `#<hash>` から現在コミットハッシュを抽出し、`git ls-remote` の結果と比較して差分があれば更新として扱う
- バージョンチェックはマニフェストごとに並列処理される (`futures::stream::buffered`)。並列度は依存数に応じて `clamp(dep_count, 1, 4)` で適応 (最大 4)。内部では各レジストリ別のセマフォ (crates.io は 1、他は 10) が効くため、レート制限は従来どおり尊重される。結果は入力順で返るため表示順は安定する
- `--age` 指定時の transitive 依存への適用方法は PM ごとに異なる:
  - **Rust (cargo)**: `cargo update` 後に Cargo.lock を走査し、age 違反を `cargo update -p <name> --precise <older_version>` で age 内の最新 stable バージョンへ差し戻す (post-install audit)。cargo の再解決で連鎖する新たな違反に備えて最大 5 回まで反復する。resolver 制約違反で差し戻し不可の場合は verbose でスキップ理由を表示して続行
  - **Node.js (pnpm v10.16+)**: `pnpm install` を `npm_config_minimum_release_age=<分>` 環境変数付きで起動する。pnpm は npm 互換の config 規約に従うため、この env var は `.npmrc` の `minimum-release-age=<分>` と等価に解釈される (公式ドキュメント: "This applies to all dependencies, including transitive ones")。pnpm v10.16 未満ではこの env var は未知設定として無視される (graceful no-op)。公式の CLI フラグは現時点で未実装 ([pnpm/pnpm#11224](https://github.com/pnpm/pnpm/issues/11224))
  - **Python (uv)**: `uv sync --exclude-newer <RFC3339>` を注入し uv ネイティブの日時フィルタを利用。transitive 含めて resolve 時に age 制約が効く
  - **その他 (npm/yarn/bun/pip/poetry/rye/pipenv/bundle/composer/gradle/swift/go)**: transitive 依存へのネイティブ age サポートが無いため direct deps のみ age 制御される。verbose モードで通知
- pnpm の fallback には既知の不具合あり: 同一 major 内の intermediate 版への fallback が失敗するケース ([pnpm/pnpm#11203](https://github.com/pnpm/pnpm/issues/11203))、`minimumReleaseAgeExclude` 除外依存の transitive が age 違反のとき `ERR_PNPM_NO_MATURE_MATCHING_VERSION` で失敗するケース ([pnpm/pnpm#11068](https://github.com/pnpm/pnpm/issues/11068)) など。transitive が基本的には守られるが、完全ではない点に注意
