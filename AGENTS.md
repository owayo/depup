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
  global_config.rs - グローバル設定ファイル (~/.config/depup/config.toml) パーサ
  osv.rs           - OSV.dev API による脆弱性チェック
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
    json_sections.rs - JSON マニフェストの依存セクション限定書き換え補助
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

- バージョン比較は数値ベースの semver 比較を使用 (文字列比較ではない)。数値コアが等しい場合、プレリリース付き (例: `1.0.0-rc.1`) は安定版 (`1.0.0`) より小さい (semver 11.4.3)。両方ともプレリリースの場合はプレリリース部の数値識別子で比較される (例: `19.3.0-canary-123 < 19.3.0-canary-456`)
- プレリリースバージョン (alpha/beta/canary/dev/rc) はデフォルトでフィルタされる
- 作者が非推奨を示すためにリリース末尾へ付与するマーカー (`-deprecated` / `-obsolete` / `-retired` / `-yanked` / `-unmaintained`) も prerelease として扱われ、デフォルト更新対象から除外される (例: `serde_yaml 0.9.34-deprecated` は 0.9.33 から更新されない)
- Go は常に pinned 扱い (`--include-pinned` 不要) だが、`// pinned` コメント付き依存は `--include-pinned` がないとスキップされる
- Ruby の `group` ブロックはネストを考慮して判定され、内側の `:development` / `:test` を抜けた後の gem は開発依存として漏れない。`platforms` / `source` 等のネストされた `do...end` ブロックもブロック種別スタックで正しく追跡する。`source do` 内の `group :development do ... end` を抜けた後の gem も開発依存として漏れない。`group :development do # comment` のようにインラインコメントが付いた group 開始行も正しくグループとして認識される。`gem "rspec", group: :test` / `groups: [:development, :test]` のような行単位の group オプションも開発依存として扱う
- Gemfile のバージョンなし `git:` / `github:` / `bitbucket:` / `gist:` / `path:` / `source:` 依存は RubyGems のレジストリ依存ではないため更新対象から除外する。バージョンが明示されている同種依存は Bundler のバージョンチェックとして解析対象にできる
- Gemfile の `gem "rack", "~> 3.0"` 形式と `gem("rack", "~> 3.0")` の括弧付きメソッド呼び出し形式はどちらも解析・更新対象とし、更新時は元の呼び出し形式を保持する
- Ruby のドット区切りプレリリース (例: `7.0.0.alpha.2`, `1.0.0.pre.1`) もパースと更新に対応する
- Gemfile の複合制約・除外制約（例: `'>= 0.18', '< 2.0'`, `'!= 2.2.4'`）は解析対象だが、安全に書き換えられないため自動更新ではエラーとして扱う
- Java/Gradle の strict 記法（例: `1.2.3!!`）は固定バージョンとして解釈され、`!!` を保持して更新される。Groovy の `group: 'x', name: 'y', version: 'z'` と Kotlin DSL の `group = "x", name = "y", version = "z"` の map 記法も解析・更新対象になる
- Gradle の rich version ブロック（例: `implementation("org.slf4j:slf4j-api") { version { strictly("[1.7, 1.8["); prefer("1.7.25"); reject("1.7.36") } }`）は `strictly` / `require` / `prefer` / `reject` を解析する。`strictly` / `require` が範囲で `prefer` がある場合は、範囲を上限制約として保持し、更新時は `prefer` の値を書き換える。`reject` に列挙されたバージョンは更新候補から除外し、`2.+` のような動的 reject も考慮する。Gradle の仕様どおり、後続の `strictly` / `require` / `prefer` 宣言は先行する reject を消す。`//` 行コメントと `/* ... */` ブロックコメント内の rich version 宣言および直接依存宣言は無視する
- Gradle の文字列記法では `group:name:version:classifier@extension` と `group:name:version@extension` を解析・更新でき、更新時は classifier / extension サフィックスを維持する
- Maven の Hard requirement (例: `[1.0]`, `[1.2.3]`, `[1.2.3.Final]`) は完全一致 (Exact) として解釈され、ブラケットを保持したまま更新される (例: `[1.0]` → `[1.5]`)。`[A,B]` のようにカンマを含むレンジ記法とは区別される
- Node/Python/Rust/PHP/Gradle の部分ワイルドカード指定（例: `1.x`, `1.x.x`, `v1.*`, `1.2.*`, `1.+`）は形を保って更新される
- 完全浮動指定（例: `*`, npm dist-tag の `latest`, Gradle の `latest.release` / `latest.integration` / `latest.milestone` / ユーザ定義 status）は意味を変えないため更新対象から除外される
- Range制約 (`>=X,<Y` / `>=X,<=Y` / `A..<B` / `A...B` / `A - B` / `[A,B)` / `[A,B]` / `[A,B[`) では上限を超えるバージョンは除外され、更新時は上限制約を維持したまま下限側のみを互換な最新バージョンへ進める (`<=` / `...` / 閉じ `]` は上限値を含む。npm/Composer の `A - B` は右辺が完全指定なら包含、`1.0 - 2.0` のような部分指定ならワイルドカード展開後の排他的上限として扱う)。上限制約が先に書かれた場合も、書き換えるのは包含下限側のみ
- 安全に書き換えられない制約（例: npm/Composer の `^1 || ^2`、`!=` を含む除外制約 `!=1.2.3` / `>=1.0, !=1.5.0, <2.0`、上限のみの `<4.0.0` / `<=2.0`、厳密な下限の `>1.0.0`、下限なし Maven 形式 `(,2.0]`、排他的下限を持つ Maven 形式 `]1.0,2.0[` / `]1.0,2.0]`）は自動更新から除外される
- Maven 形式の qualifier 付き上限（例: `[1.0,2.0.Final)`, `[1.0,2.0-beta1-SNAPSHOT)`）も上限制約として解釈される。Gradle のバージョン部は `.`, `-`, `_`, `+` 区切りと `1a1` のような英数字混在パートを許容する
- `package.json` の更新は `dependencies` / `devDependencies` / `peerDependencies` / `optionalDependencies` に限定し、`overrides` 等は書き換えない。`composer.json` の更新は `require` / `require-dev` に限定し、`replace` / `provide` / `conflict` 等は書き換えない
- Composer の platform package (`php`, `hhvm`, `php-*`, `ext-*`, `lib-*`, `composer*`) は更新対象から除外する
- Cargo workspace の `[workspace] members` に指定されたメンバークレートの Cargo.toml も自動検出
- Cargo.toml の `[dependencies.<name>]` テーブル形式の更新では、`serde` を更新する際に `serde_json` のような名前プレフィックスを共有するパッケージへ誤マッチしない (パッケージ名の直後は `]` か空白のみを許容)
- Cargo.toml の `package = "actual-crate"` 付きリネーム依存では、レジストリ取得には実パッケージ名を使い、書き戻しにはマニフェスト上の依存キーを使う。`--only` / `--exclude` はどちらの名前でも一致する
- Tauriプロジェクトでは npm/crate のバージョンを自動同期
- Swift は GitHub Tags API を使用 (`GITHUB_TOKEN`/`GH_TOKEN` で認証可能)。GitHub Tags API はリリース日を返さないため、各バージョンの `released_at` には UNIX_EPOCH を使う (= 「十分古い」として扱う)。これにより `--age` 指定時でも Swift パッケージの更新が抑制されない
- Swift の GitHub タグは `v1.2.3` と `V1.2.3` の両方を認識する
- Swift の非 GitHub URL はスキップされる (警告なし)
- Swift の `branch:` / `revision:` 依存はバージョンなしとしてスキップ
- Swift の `Package.swift` では `//` 行コメントと `/* ... */` ブロックコメント内の依存宣言をスキップする
- Swift (SPM) は semver 2.0.0 準拠のためプレリリース識別子 (例: `1.0.0-beta.1`) とビルドメタデータ (例: `1.0.0+build.123`)、両者の組み合わせ (例: `1.0.0-rc.1+sha.abc`) を `from:` / `exact:` / `.upToNextMajor` / `.upToNextMinor` のいずれでも解析・更新できる
- Rust (Cargo) の演算子は `>= 1.2.3` のようにスペースを含む形式も対応し、`>=1.0, <2.0, >=1.0.100` のような3個以上の複数 comparison requirement も Range として解析する
- Cargo.toml の通常依存・inline table・複数行テーブルの更新では TOML の単一引用符 (`'1.0.0'`) も保持する。通常依存・inline table の書き換えは `[dependencies]` / `[dev-dependencies]` / `[build-dependencies]` / `[workspace.dependencies]` / target 固有依存セクションに限定し、`[package.metadata]` 等の依存セクション外の同名キーは書き換えない
- Python の Range 制約は単一セグメントバージョン（例: `>=3,<4`）も正しくパースする。PEP 440 の compatible release 句は `~=1.2` / `~=1.2.3` を扱い、仕様上無効な単一セグメント形式 `~=1` はスキップする。PEP 508 の括弧付き versionspec（例: `requests (>=2.28,<3); python_version < "3.12"`）、空白を含む extras（例: `coverage [toml] >=7,<8`）、末尾カンマ付き version list（例: `paramiko>=3.5,<4,`）も元の形を保って更新する。`pyproject.toml` の Poetry 形式・inline table・PEP 508 配列要素では TOML の単一引用符も保持して更新する
- Go の `replace` ディレクティブ（単一行・ブロック形式とも）はパースと更新の両方でスキップされる
- Go の `exclude` ディレクティブ（単一行・ブロック形式とも）はパースと更新の両方でスキップされる
- go.mod の `) // comment` のようなコメント付きブロック終端も、`require` / `replace` / `exclude` ブロックの終端として扱う
- go.mod の quoted module path / version（例: `require "golang.org/x/text" "v0.14.0"`）も解析・更新し、引用符を維持する
- Maven Central のクエリはグループID/アーティファクトIDの文字種を検証し、不正な文字によるURLインジェクションを防止する
- npm/Composer/Cargo は semver の prerelease (`-...`) と build metadata (`+...`) を同時に含むバージョン（例: `^1.2.3-rc.1+build123`）も正しくパースする。ビルドメタデータはバージョン比較時には無視される (semver 仕様)
- Rust (Cargo) の git 依存 (`{ git = "...", branch/tag/rev = "..." }` / 省略形) を検出し、`git ls-remote` でリモート HEAD / タグを取得して更新判定する。branch / 省略形 (デフォルトブランチ) は最新コミットへ更新 (Cargo.toml は書き換えず、Cargo.lock を `--install` の `cargo update` で再解決)、tag は最新 semver タグへ更新 (Cargo.toml の tag 文字列を書き換え)、rev は常にスキップ (pinned 扱い)
- Cargo.lock の git source 末尾 `#<hash>` から現在コミットハッシュを抽出し、`git ls-remote` の結果と比較して差分があれば更新として扱う
- バージョンチェックはマニフェストごとに並列処理される (`futures::stream::buffered`)。並列度は依存数に応じて `clamp(dep_count, 1, 4)` で適応 (最大 4)。内部では各レジストリ別のセマフォ (crates.io は 1、他は 10) が効くため、レート制限は従来どおり尊重される。結果は入力順で返るため表示順は安定する
- `--only` が指定されている場合は `--exclude` より優先される。Cargo リネーム依存では実パッケージ名・マニフェスト名のどちらで指定しても同じ優先順位で判定する
- `.depup` にルートとサブディレクトリが両方含まれる場合、`--install` と Rust の `--age --install` 後処理は、更新されたマニフェストに最も近い（最も深い）対象ディレクトリで実行する
- `--age` 指定時の transitive 依存への適用方法は PM ごとに異なる:
  - **Rust (cargo)**: `cargo update` 後に Cargo.lock を走査し、age 違反を `cargo update -p <name> --precise <older_version>` で age 内の最新 stable バージョンへ差し戻す (post-install audit)。cargo の再解決で連鎖する新たな違反に備えて最大 5 回まで反復する。resolver 制約違反で差し戻し不可の場合は verbose でスキップ理由を表示して続行
  - **Node.js (pnpm v10.16+)**: `pnpm install` を `npm_config_minimum_release_age=<分>` 環境変数付きで起動する。pnpm は npm 互換の config 規約に従うため、この env var は `.npmrc` の `minimum-release-age=<分>` と等価に解釈される (公式ドキュメント: "This applies to all dependencies, including transitive ones")。pnpm v10.16 未満ではこの env var は未知設定として無視される (graceful no-op)。公式の CLI フラグは現時点で未実装 ([pnpm/pnpm#11224](https://github.com/pnpm/pnpm/issues/11224))
  - **Python (uv)**: `uv sync --exclude-newer <RFC3339>` を注入し uv ネイティブの日時フィルタを利用。transitive 含めて resolve 時に age 制約が効く
  - **その他 (npm/yarn/bun/pip/poetry/rye/pipenv/bundle/composer/gradle/swift/go)**: transitive 依存へのネイティブ age サポートが無いため direct deps のみ age 制御される。verbose モードで通知
- pnpm の fallback には既知の不具合あり: 同一 major 内の intermediate 版への fallback が失敗するケース ([pnpm/pnpm#11203](https://github.com/pnpm/pnpm/issues/11203))、`minimumReleaseAgeExclude` 除外依存の transitive が age 違反のとき `ERR_PNPM_NO_MATURE_MATCHING_VERSION` で失敗するケース ([pnpm/pnpm#11068](https://github.com/pnpm/pnpm/issues/11068)) など。transitive が基本的には守られるが、完全ではない点に注意
- `--age` のデフォルトは `1w` (組み込みデフォルト)。グローバル設定 `~/.config/depup/config.toml` の `age = "1w"` 等で上書き可能。優先順位は `--no-age` > `--age` > グローバル設定 > 組み込みデフォルト (1w)。`--age` と `--no-age` は同時指定できない (clap の conflicts_with)
- グローバル設定ファイル `~/.config/depup/config.toml` は初回実行時に自動生成される (claw-hooks 方式)。雛形は組み込みデフォルトと一致するキーをコメント付きで書き出す (`age = "1w"`、`osv` はコメントアウト)。生成失敗時 (権限など) は警告を出して処理を継続し、組み込みデフォルトで動作する。既存ファイルは絶対に上書きしない
- `--max-change <LEVEL>` (patch / minor / major) で許容する bump レベルを制限。現在版と候補版を semver の数値コアで比較し、`Patch < Minor < Major` の順序で `level <= max` を通す。除外された候補がある場合は `SkipReason::ChangeLevelLimited(level)` で skip。グローバル設定 `max_change = "minor"` で常時設定可能。優先順位は CLI > config > 組み込みデフォルト (制限なし)
- age 解決の優先順位 (build_filter で確定): プロジェクト `minimumReleaseAge` (pnpm / bun) > CLI `--age` > `--no-age` > グローバル設定 `age` > 組み込みデフォルト 1w。`minimumReleaseAge` はプロジェクトポリシーとして CLI を上書きする。CLI が無視される場合は `⚠ --age ignored: project's minimumReleaseAge (N days from <source>) takes precedence` を黄色で stderr に通知。pnpm と bun の両方が値を持つ場合はより厳しい (max) を採用。bun は `bunfig.toml` の `[install] minimumReleaseAge` (秒単位の整数)。`main.rs` では age の resolve はしない (orchestrator の `with_global_config` で渡し、`build_filter` で解決)
- `--osv` で OSV.dev API による脆弱性チェックを有効化。`judge_with_osv` は採用しようとした候補だけを `https://api.osv.dev/v1/query` に POST し、`vulns` が空でなければ その version を candidate から除外して再 judge → 次に古い候補へフォールバックする。全 candidate を網羅的にチェックする方式は採らない (1000+ バージョンを持つ `@angular/*` 等で実用速度を確保するため)。通常 1 依存あたり 1〜2 API call で完了する。API エラー時は元の候補を採用して `--verbose` で警告。Swift は OSV ecosystem 未対応のためスキップ。グローバル設定 `osv = true` で常時有効化可能。優先順位は `--no-osv` > `--osv` > グローバル設定 > 組み込みデフォルト (false)。OSV API は認証トークン不要
