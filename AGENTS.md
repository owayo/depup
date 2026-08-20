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
    package_json.rs  - Node.js パーサ (Bun catalogs 対応)
    pyproject_toml.rs - Python パーサ
    cargo_toml.rs    - Rust パーサ (git 依存検出対応)
    cargo_lock.rs    - Cargo.lock から git 依存の現在 commit 抽出
    go_mod.rs        - Go パーサ
    gemfile.rs       - Ruby パーサ
    composer_json.rs - PHP パーサ
    gradle.rs        - Java パーサ
    gradle_version_catalog.rs - Gradle version catalog パーサ
    json_sections.rs - JSON マニフェストの依存セクション限定書き換え補助 (ネストした object 範囲の抽出含む)
    line_utils.rs    - 行末改行分離 (split_line_ending)・クォート判定 (captured_quote_and_version)・クォート考慮の # 行コメント除去 (strip_hash_line_comment、HashCommentMode でバックスラッシュエスケープ解釈を Gemfile/TOML で切替)・TOML セクションヘッダ字句解析 (parse_toml_section_header、`[key]` / `[[key]]` / ヘッダ内空白 / 行末コメントを解釈) の共通ヘルパ。CRLF 保持と TOML クォート種別判定・セクション追跡の単一情報源で、cargo_toml / gemfile / gradle / gradle_version_catalog / pyproject_toml が共用する
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
| Node.js | package.json (Bun catalogs 含む) | npm |
| Python | pyproject.toml | PyPI |
| Rust | Cargo.toml (workspace members 自動検出) | crates.io |
| Go | go.mod | Go Proxy |
| Ruby | Gemfile | RubyGems |
| PHP | composer.json | Packagist |
| Java | build.gradle / build.gradle.kts / gradle/*.versions.toml | Maven Central |
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
- E2E テストは Cargo が提供する `CARGO_BIN_EXE_depup` のコンパイル済みバイナリを使う。テストプロセス内から `cargo build` を起動すると、親の `cargo test` が保持する Cargo ロックと循環待ちになり全テストが停止するため、独自ビルドを行わない

## Important Notes

- 更新候補の比較はエコシステム別に行う。Node/Rust/Go/Swift は SemVer 2.0.0 を使い、純粋に数値だけの `-` サフィックス (`1.0.0-1`) もプレリリースとして安定版より小さく扱う。優先順位の比較では `semver::Version::cmp_precedence` を使って build metadata (`+...`) を無視し、`1.1.3` と `1.1.3+spec-1.1.0` の差だけで更新を発生させない。通常の `Ord` は全順序のため build metadata も比較するので更新判定には使わない。Python は `pep440_rs` による PEP 440 正規化・順序付け（代替綴り、epoch、pre/dev/post/local を含む）、Ruby は RubyGems の数値・英字セグメント順を使い、英字またはハイフンを含む版をプレリリースとして安定版利用者の候補から除外する。PHP は Composer の patch alias (`-p1` / `-pl1` / `-patch1`)、Java は Gradle 公式の version ordering（区切り同値、英数字分割、追加パート、special qualifier 順）を使う。数値コア・epoch・pre/post/local の数値識別子は `u64` へ変換せず任意桁の10進文字列として比較し、巨大な数値でもオーバーフローによるダウングレードを起こさない
- Node は node-semver 互換の `~>1.2.3` を Tilde として受理し、更新後も `~>` を保持する。Composer は `=1.2.3` / `==1.2.3` を Exact として演算子を保持し、`<>1.2.3` は除外 Range として surface するが安全に書き換えられないため Skip する。Gradle rich version の `reject` は固定値・動的指定 (`2.+`) に加えて Maven 形式レンジ (`[1.5,1.9)`) も候補除外へ反映する
- `.depup` の各行は配置ディレクトリ配下の相対パスだけを許可する。絶対パス、`..`、外部ディレクトリへ解決される symlink は設定エラーとして拒否し、`--install` を含む処理がプロジェクト外へ出ないようにする
- バージョンキャッシュはキー単位ロックで single-flight 化し、同一言語・同一パッケージの並行要求は1回のレジストリアクセスへ集約する。異なるパッケージの並列性は維持する
- バージョン文字列の比較は多バイト UTF-8 に対して panic セーフ。数値プレフィックスに続く英字/多バイトセグメント (例: `1.0.0.0abcé`) の post 判定を `post` のバイト列比較で行い、文字境界の途中でスライスしない。以前は `rest[..4]` / `follow[..4]` の文字列スライスが `é` 等の境界で panic し、レジストリやマニフェストが非 ASCII のバージョン文字列を 1 件返すだけで該当依存だけでなくプロセス全体がクラッシュしていた
- レンジ制約の比較基準 (judge が使う現在版) は comparator の記述順に依存せず包含下限を採用する (Node/Python/Rust/PHP)。`<1.5, >=1.2.2` のように上限を先に書いても下限 `1.2.2` を基準にする。書き換え側 (`format_range_like`) と同じトークン探索 (`range_lower_bound_version`) を共有するため、judge の比較基準と writer の書き換え対象が必ず一致する。包含下限が無い場合 (厳密下限 `>1.0` のみ等) は従来の先頭トークン抽出にフォールバックする。以前は先頭トークン (=上限を先に書くと上限) を基準にして judge が AlreadyLatest と誤判定し、有効な更新を取りこぼしていた
- Python パーサは数字を含まない version 部 (例: `==hello` / `>=foo` / `^foo` / `>=local-only`) を `None` で弾く (以前は `version=""` の VersionSpec を silent に受理しており、後段の比較で意図しない更新候補選択を引き起こす可能性があった)。`==1.0` / `==v1.0` / `==1.0a1` のような数字を含む有効入力は引き続き受理する
- プレリリースバージョン (alpha/beta/canary/dev/rc) はデフォルトでフィルタされる。`-rc.1` のようなセパレータ付き形式に加え、PEP 440 のセパレータなし形式 (`2.0.0rc1` / `1.0rc1` / `1.0.0a1` / `1.0.0b1`) も検出して除外する (安定版利用者が rc 版へ誤更新されるのを防ぐ)
- Python パーサは比較用バージョンに PEP 440 のプレリリース部とエポックを保持する (`>=2.0.0rc1` の現在版は `2.0.0rc1`)。これにより rc 利用者が安定版 (`2.0.0`) へ正しく昇格でき、「現在版がプレリリースなら候補にプレリリースを残す」ルールが Python でも機能する
- Python 依存では PEP 440 local version (`+local`) を semver の build metadata とは区別する。`==1.0+cu121` / `!=1.0+local1` は local label を保持して解析し、候補選択では `1.0+local > 1.0`、`1.0+1 > 1.0+abc`、`1.0+abc.2 > 1.0+abc.1` のように PEP 440 順で比較する。PEP 440 で local version が許可されない ordered/compatible 指定 (`>=1.0+local` / `~=1.0+local` / `>=1.0+local,<2.0`) は安全側でスキップする
- 作者が非推奨を示すためにリリース末尾へ付与するマーカー (`-deprecated` / `-obsolete` / `-retired` / `-yanked` / `-unmaintained`) も prerelease として扱われ、デフォルト更新対象から除外される (例: `serde_yaml 0.9.34-deprecated` は 0.9.33 から更新されない)
- JVM 系の milestone 版 (`4.0.0-M1` / 旧 Spring Boot のドット区切り `2.0.0.M1` / 綴りきりの `-milestone1`) も prerelease として扱い、安定版利用者の更新候補から除外する。Node/Rust/Go/Swift は semver 判定で `-` サフィックスを一律プレリリースとするが、**Java / PHP は識別子リストで判定する**ため `M<数字>` を明示的に列挙しないと安定版扱いになり、`assertj-core 3.24.2` → `4.0.0-M1`、`junit-bom 5.10.0` → `5.13.0-M3`、`spring-core 5.3.23` → `7.0.0-M6` のように milestone へ誤更新されていた。判定は「`m` の直後が数字のトークン」に限定するため、`.Final` / `-jre` / `-android` / `.RELEASE` / `.GA` / `-SP1` のような JVM の**安定版 qualifier** や `-macos1` / `-m2m` は巻き込まない (Java で「qualifier があれば一律プレリリース」とすると安定版が全滅する)
- Go は常に pinned 扱い (`--include-pinned` 不要) だが、`// pinned` コメント付き依存は `--include-pinned` がないとスキップされる。`// pinned` の検出は語順非依存で、`// indirect; pinned` のように `pinned` が `//` 直後でなくコメント内のどこにあっても認識する (`// indirect` 判定が `contains("indirect")` で語順非依存なのと整合させた)。単語境界判定により `unpinned` / `repinned` は pinned 指定として誤認しない
- `go.mod` の単一行・ブロック形式の `require` 更新は `split_line_ending` で元の行末を退避・再付与し、LF / CRLF と最終行の改行有無を維持する
- Ruby の `group` ブロックはネストを考慮して判定され、内側の `:development` / `:test` を抜けた後の gem は開発依存として漏れない。`platforms` / `source` 等のネストされた `do...end` ブロックもブロック種別スタックで正しく追跡する。`source do` 内の `group :development do ... end` を抜けた後の gem も開発依存として漏れない。`group :development do # comment` のようにインラインコメントが付いた group 開始行も正しくグループとして認識される。`gem "rspec", group: :test` / `groups: [:development, :test]` のような行単位の group オプションも開発依存として扱う
- Gemfile のバージョンなし `git:` / `github:` / `bitbucket:` / `gist:` / `path:` / `source:` 依存は RubyGems のレジストリ依存ではないため更新対象から除外する。バージョンが明示されている同種依存は Bundler の gemspec バージョンチェックとして解析・更新対象にでき、source オプションは保持する。オプションキーは Ruby の 2 通りの綴り (`git: '...'` と `:git => '...'` の hash-rocket 記法) の両方を検出する (以前は `git:` 綴りしか見ておらず、git 依存を更新候補として報告した上で書き込み時に必ず失敗していた)
- Gemfile の `git ... do` / `github ... do` / `path ... do` / `source ... do` ブロック内の gem も、行オプション形式と同じくレジストリ外依存として更新対象から除外する (git/path なら `bundle install` が壊れ、private source なら同名の公開 gem の版が入る)。`platforms` / `install_if` のような通常のブロックは従来どおり対象
- Gemfile の引数が次行へ続く宣言 (`gem "devise",` で行が終わる形) は、その行だけでは版を決められないため「バージョンなしのレジストリ依存」と誤報告せず取りこぼす (以前は更新を報告した上で書き込みが必ず失敗していた)。同一行に version がある場合は末尾に `,` が続いても従来どおり更新対象
- Gemfile の `gem "rack", "~> 3.0"` 形式と `gem("rack", "~> 3.0")` の括弧付きメソッド呼び出し形式はどちらも解析・更新対象とし、更新時は元の呼び出し形式を保持する
- Gemfile の行末条件修飾子 (`gem 'wdm', '>= 0.1.0' if Gem.win_platform?` / `gem 'tzinfo-data', '~> 1.2' unless ...`) 付きの行も version 制約を取りこぼさずに解析・更新し、修飾子を保持する (括弧付き `gem("sqlite3", "~> 1.4") if ...` 形式も対応)。以前は末尾の `if`/`unless` で正規表現がバックトラックして version を落とし Any と誤分類していた
- Ruby のドット区切りプレリリース (例: `7.0.0.alpha.2`, `1.0.0.pre.1`) もパースと更新に対応する
- Gemfile の複合制約・除外制約（例: `'>= 0.18', '< 2.0'`, `'!= 2.2.4'`）は解析対象だが、安全に書き換えられないため自動更新ではエラーとして扱う
- Java/Gradle の strict 記法は固定値 (`1.2.3!!`) に加え、動的プレフィックス (`5.3.+!!`) と prefer なしの範囲 (`[1.7, 1.8[!!`) も解釈し、`!!` と元の制約形を保持して更新される。prefer 付き strict 範囲 (`[1.7, 1.8[!!1.7.25`) も従来どおり対象。Groovy の `group: 'x', name: 'y', version: 'z'` と Kotlin DSL の `group = "x", name = "y", version = "z"` の map 記法も解析・更新対象になる
- Gradle の rich version ブロック（例: `implementation("org.slf4j:slf4j-api") { version { strictly("[1.7, 1.8["); prefer("1.7.25"); reject("1.7.36") } }`）は `strictly` / `require` / `prefer` / `reject` を解析する。`group:name:[1.7, 1.8[!!1.7.25` のような文字列記法の strict range + prefer 短縮構文も解析する。`strictly` / `require` が範囲で `prefer` がある場合は、範囲を上限制約として保持し、更新時は `prefer` の値を書き換える。`reject` に列挙されたバージョンは更新候補から除外し、`2.+` のような動的 reject も考慮する。Gradle の仕様どおり、後続の `strictly` / `require` / `prefer` 宣言は先行する reject を消す。`//` 行コメントと `/* ... */` ブロックコメント内の rich version 宣言および直接依存宣言は無視する。`rejectAll()` が宣言された依存は全バージョン拒否として更新対象から除外する (version catalog の `rejectAll = true` と挙動を揃え、拒否制約を無視した誤更新を防ぐ安全側のスキップ)
- Gradle の文字列記法では `group:name:version:classifier@extension` と `group:name:version@extension` を解析・更新でき、更新時は classifier / extension サフィックスを維持する
- Gradle の宣言ラッパ `platform(...)` / `enforcedPlatform(...)` / `testFixtures(...)` を挟んだ依存も解析・更新できる (`implementation platform('com.google.cloud:libraries-bom:26.1.0')` / `implementation(platform("org.springframework.boot:spring-boot-dependencies:3.2.0"))` / `testImplementation(platform("org.junit:junit-bom:5.10.0"))`)。BOM は推移依存のバージョンを一括決定する要となる宣言なので、取りこぼすとプロジェクト全体のバージョンが古いまま放置される。ラッパは configuration 名の後に来るため、素朴な正規表現では `platform` 自体が configuration 名と解釈されて dev 判定 (`testImplementation` か否か) も壊れる。共通の `DEP_WRAPPER` パターンを文字列記法 / map 記法 / 変数展開 / rich version の各正規表現で共有して防ぐ
- Gradle の `ext.<name> = '...'` / `project.ext.<name> = "..."` のドット代入も変数として解決・書き戻しできる (`ext { ... }` ブロック形式と意味が同じなのに挙動が割れていた)。`${Versions.retrofit}` / `${rootProject.ext.springVersion}` のような修飾付き変数参照も最終セグメントで解決する。ただし同じ短名が異なる値で複数定義されている場合 (`object Versions` と `object Legacy` が同名の定数を持つ等) は別オブジェクトの値を拾う誤更新を避けて解決しない (安全側のスキップ)
- Gradle version catalog (`gradle/*.versions.toml`) は `[libraries]` の `alias = "group:name:version"`、`module = "group:name"`、`group` / `name` / `version`、`version.ref` を解析・更新できる。`[versions]` 参照先も更新し、rich version table の `strictly` / `require` / `prefer` / `reject` / `rejectAll` も Gradle ファイル本体と同じルールで扱う。`[plugins]` は Gradle plugin ID で Maven Central 座標と一致しないため更新対象から除外する
- Gradle の変数定義 (`def x = '...'` / `val x = "..."`) / rich version ブロック / version catalog (`*.versions.toml`) 経由の更新でも CRLF (`\r\n`) を保持する。以前はこれらの経路が `content.lines()` + `join("\n")` でファイル全行を LF 化しており、文字列記法・map 記法経路 (`split_inclusive` で CRLF 保持) と挙動が食い違っていた (共通ヘルパ `split_line_ending` で行末を退避・再付与する)
- Maven の Hard requirement (例: `[1.0]`, `[1.2.3]`, `[1.2.3.Final]`) は完全一致 (Exact) として解釈され、ブラケットを保持したまま更新される (例: `[1.0]` → `[1.5]`)。`[A,B]` のようにカンマを含むレンジ記法とは区別される
- Node/Python/Rust/PHP/Gradle の部分ワイルドカード指定（例: `1.x`, `1.x.x`, `v1.*`, `1.2.*`, `1.+`）は形を保って更新される。npm の caret/tilde/equality + x-range（例: `^1.x`, `~1.2.x`, `^1.2.*`, `=1.x`）も演算子を保持したワイルドカードとして認識し、形を保って更新される（例: `^1.x` → `^2.x`、`=1.x` → `=2.x`）。npm の partial comparator (`=1.2` / `=1`) は node-semver の部分バージョン規則に従う Range として扱い、固定バージョンではなく `=` とセグメント数を保って更新する (`=1.2` → `=2.3`)。`^1` / `^1.2.3` のようにワイルドカード文字を含まない指定は従来どおり Caret/Tilde として扱う。Cargo (Rust) も `=1.*` / `^1.*` / `~1.x` のような演算子付きワイルドカードを semver crate と同様に valid として認識し、演算子を保持して形を保ったまま更新する（例: `^1.*` → `^2.*`。以前は parse で取りこぼし黙ってスキップしていた）。Composer (PHP) は小文字 `v` に加えて大文字 `V` をワイルドカード (`V1.*` / `V1.x`) だけでなく Exact / Caret / Tilde / 比較演算子 / ハイフンレンジを含む全構文 (`V1.2.3` / `^V1.2.3` / `>=V1.0` 等) で受理する (composer/semver が v/V を大小問わず許容するため。バージョンコアのパターンは `PHP_VERSION_CORE` 定数に集約し、Node の `NODE_VERSION_PATTERN` と同様に全正規表現で共有することで定義間の不整合を防ぐ)。v/V 接頭辞は比較・更新時に正規化され、ワイルドカード以外の更新後表記では除去される (Packagist の正規化形に整合。小文字 `v` も従来同様)
- Tilde 制約 (`~` / RubyGems の `~>`) は**元の指定と同じセグメント数**を保って更新する (`~2.0` → `~3.10`、`~1` → `~3`、`~1.2.3` → `~1.8.9`)。Tilde の許容幅はセグメント数で決まるため、レジストリの完全版 (3 セグメント) をそのまま書き戻すと制約が黙って狭まる: Composer の `~2.0` は `>=2.0 <3.0` だが `~3.10.5` にすると `<3.11` へ、RubyGems の `~> 7.0` は `>= 7.0, < 8.0` だが `~> 7.1.3` にすると `< 7.2` へ縮み、以後の `composer update` / `bundle update` がマイナー系列を跨げなくなる。npm/Cargo/Poetry でも `~1` (= `>=1.0.0 <2.0.0`) が `~2.5.3` (= `<2.6.0`) へ縮む。更新先が短い場合は 0 埋めして幅を保ち (`~> 1.2.3.4` + `1.9` → `~> 1.9.0.0`)、更新先がプレリリース / ビルドメタデータを含む場合は識別子を落とさないよう完全版を使う (`~1.2` + `2.0.0-rc.1` → `~2.0.0-rc.1`)。**セグメント数の根拠には比較用の `version` ではなく生表記 (`raw`) を使う** — Node のパーサだけが比較用バージョンを 3 セグメントへ 0 埋め正規化する (`~10.3` → `version = "10.3.0"`) ため、`version` から数えると Node だけ保持が効かず、しかも `VersionSpec` を手組みするユニットテストではこの穴を検出できない (各パーサ経由の回帰テストを併置している)
- 完全浮動指定（例: `*`, npm dist-tag の `latest`, Gradle の `latest.release` / `latest.integration` / `latest.milestone` / ユーザ定義 status）は意味を変えないため更新対象から除外される。Composer (PHP) の `*.*` / `v*` / `V*` / `x.x` のように数値アンカーを持たない多セグメントワイルドカード、および Java/Gradle の `[,]` / `(,)` のように下限・上限とも空の Maven レンジも同様に更新対象外として弾く (受理すると version 空の Wildcard/Range が作られ、phantom update や「常に古い」と誤判定する原因になる)
- ワイルドカード文字 (`x`/`X`/`*`) が現れた後ろに数値セグメントが続く形式 (`1.x.3` や `^x.0.0` のような node-semver / semver で invalid な x-range) はパース時点で None で弾く。受理して `version="0.0.0"` のような捏造値で比較すると、誤った更新候補を選んだり `format_updated` で `2.x.4` のような不正出力を出す可能性があるため (Rust の semver crate と同じ判定)
- npm の semver トークンは prerelease / build metadata 識別子を検証し、アンダースコアを含む識別子 (`1.2.3-rc_1`)、空識別子 (`1.2.3-alpha..1`)、先頭ゼロを持つ数値 prerelease 識別子 (`1.2.3-01`) を parse 時点で None として弾く。node-semver に存在しない `!=` comparator を含む comparator set も Node では更新対象にしない
- Range制約 (`>=X,<Y` / `>=X,<=Y` / `A..<B` / `A...B` / `A - B` / `[A,B)` / `[A,B]` / `[A,B[`) では上限を超えるバージョンは除外され、更新時は上限制約を維持したまま下限側のみを互換な最新バージョンへ進める (`<=` / `...` / 閉じ `]` は上限値を含む。npm/Composer の `A - B` は右辺が完全指定なら包含、`1.0 - 2.0` のような部分指定ならワイルドカード展開後の排他的上限として扱う)。npm の comparator set では `1.2 <2.0.0` のような bare partial lower bound も Range として扱い、下限更新時は partial の形を保持する (`1.2 <2.0.0` → `1.9 <2.0.0`)。上限制約が先に書かれた場合も、書き換えるのは包含下限側のみ。PEP 440 の prefix-match wildcard (`==1.2.*` は `>=1.2.0,<1.3.0` 相当) は `.*` 直前のリリースセグメントを +1 した排他的上限として扱い、major/minor を跨ぐ誤更新を防ぐ (`==1.2.*` は 2.x 系へ更新されず 1.2 系内に留まる。`==1.*` は `<2`、epoch 付き `1!2.3.*` は epoch を保持して `<1!2.4`)。`!=1.2.*` は除外制約であり上限ではないため対象外
- 安全に書き換えられない制約（例: npm/Composer の `^1 || ^2`、Composer の後方互換表記 `^1 | ^2`、`!=` を含む除外制約 `!=1.2.3` / `>=1.0, !=1.5.0, <2.0`、上限のみの `<4.0.0` / `<=2.0`、厳密な下限の `>1.0.0`、下限なし Maven 形式 `(,2.0]`、排他的下限を持つ Maven 形式 `]1.0,2.0[` / `]1.0,2.0]`）は自動更新から除外される。Composer (composer/semver) は not-equal を `!=` と `<>` の両方で綴れる (演算子パターン `(<>|!=|>=?|<=?|==?)`) ため、`>=1.0 <>1.5.0 <2.0` / `>=1.0,<>1.5.0,<2.0` のような `<>` 除外制約も `!=` と同様に安全側でスキップする (`contains_not_equal_operator` が両綴りを検出。以前は `<>` だけ下限を書き換えて、除外バージョンを選ぶと `>=1.5.0 <>1.5.0 <2.0` のような充足不能な制約を書き戻す恐れがあった)
- Maven 形式の qualifier 付き上限（例: `[1.0,2.0.Final)`, `[1.0,2.0-beta1-SNAPSHOT)`）も上限制約として解釈される。Gradle のバージョン部は `.`, `-`, `_`, `+` 区切りと `1a1` のような英数字混在パートを許容する
- `package.json` の更新は `dependencies` / `devDependencies` / `peerDependencies` / `optionalDependencies` に限定し、`overrides` 等は書き換えない。`composer.json` の更新は `require` / `require-dev` に限定し、`replace` / `provide` / `conflict` 等は書き換えない
- Composer の platform package (`php`, `hhvm`, `php-*`, `ext-*`, `lib-*`, `composer*`) は更新対象から除外する
- Composer/Packagist は `composer/semver` の VersionParser に従い、1〜4 セグメントの数値バージョン（例: `1.2.3.4`, `^1.0.0.0`, `~3.4.5.6`, `1.0.0.*`）も valid 扱いするため、PHP パーサは Caret/Tilde/比較演算子/ワイルドカード/固定すべての形式で 4 セグメントまでパース・更新できる（5 セグメント以上は invalid として除外）
- Cargo workspace の `[workspace] members` に指定されたメンバークレートの Cargo.toml も自動検出
- Cargo.toml の `[dependencies.<name>]` テーブル形式の更新では、`serde` を更新する際に `serde_json` のような名前プレフィックスを共有するパッケージへ誤マッチしない (パッケージ名の直後は `]` か空白のみを許容)
- Cargo.toml の `package = "actual-crate"` 付きリネーム依存では、レジストリ取得には実パッケージ名を使い、書き戻しにはマニフェスト上の依存キーを使う。`--only` / `--exclude` はどちらの名前でも一致する
- Cargo.toml の dotted key 形式 (`tokio.version = "1.38"`) も解析・更新できる。toml クレートは dotted key を inline table と同じ構造へ畳むため parse は依存として surface しており、書き換え側が対応していないと「更新あり」と報告した後に書き込みが失敗して report/apply が矛盾する。書き換えは依存セクション内に限定し、`[package.metadata.custom]` 等の同名 dotted key は触らない
- Cargo.toml の path 依存 (`{ path = "../common" }`) は、publish 用に `version` を併記していても更新対象から除外する。ローカルクレートで解決されるため crates.io の最新版へ引き上げると実バージョンが要求を満たさなくなり `cargo build` が壊れる (crates.io に同名クレートが実在する場合は無関係なクレートの版で上書きされる)
- Cargo.toml の `registry = "..."` 付き依存は、`crates-io` 以外のレジストリであれば crates.io の候補で誤更新しないよう更新対象から除外する
- Tauriプロジェクトでは npm/crate のバージョンを自動同期
- Swift は GitHub Tags API を使用 (`GITHUB_TOKEN`/`GH_TOKEN` で認証可能)。GitHub Tags API はリリース日を返さないため、各バージョンの `released_at` には UNIX_EPOCH を使う (= 「十分古い」として扱う)。これにより `--age` 指定時でも Swift パッケージの更新が抑制されない
- Swift の GitHub タグは `v1.2.3` と `V1.2.3` の両方を認識する
- Swift の GitHub URL は HTTPS、scp 形式 (`git@github.com:owner/repo.git`)、標準 SSH (`ssh://git@github.com/owner/repo.git`)、GitHub の SSH over 443 (`ssh://git@ssh.github.com:443/owner/repo.git`) を owner/repo へ正規化する。`github.com.evil` 等の類似ホストは拒否する
- Swift の非 GitHub URL はスキップされる (警告なし)
- Swift の `branch:` / `revision:` 依存はバージョンなしとしてスキップ
- Swift の `Package.swift` では `//` 行コメントと `/* ... */` ブロックコメント内の依存宣言をスキップする
- Swift (SPM) は semver 2.0.0 準拠のため、`Package.swift` の version requirement は 3 セグメント (`X.Y.Z`) 必須で、数値識別子の先頭ゼロを弾く。プレリリース識別子 (例: `1.0.0-beta.1`) とビルドメタデータ (例: `1.0.0+build.123`)、両者の組み合わせ (例: `1.0.0-rc.1+sha.abc`) を `from:` / `exact:` / `.upToNextMajor` / `.upToNextMinor` のいずれでも解析・更新できる。GitHub Tags API からのタグ取得 (`github_tags.rs` の `SEMVER_RE`) は `v1.0.0-beta.1` / `1.0.0+build.123` 等のタグを取得するが、`Package.swift` 側の requirement 文字列では `v` 接頭辞付きや 2/4 セグメントの非 SemVer はスキップする。安定版/プレリリースの選別は他レジストリ (npm/PyPI 等) と同様に `UpdateJudge::stable_candidates` へ委ねる (現在版が安定版ならデフォルトでプレリリースを除外、現在版がプレリリースなら候補に残す)。末尾の `-`/`+` や `alpha..1` のような空識別子は弾く
- Swift の `.package(...)` は `name:` パラメータが `url:` の前にある正規形 (`.package(name:, url:, from:)`) のみ対応する。`name:` が `url:` の後にある非正規な引数順 (`.package(url:, name:, from:)`) は parse でスキップされる (`name:` は SPM 5.5 で非推奨のため。誤更新ではなく安全側の取りこぼし)
- Swift の `.package(...)` は version requirement の後ろに続く追加引数 (`traits: [...]` (SPM 6.1 の Package Traits) / `moduleAliases: [...]`) があっても解析・更新できる (version requirement だけを置換し、追加引数は保持する)。Swift Package Registry の `id:` 依存 (`.package(id: "scope.name", from: ...)`) は registry API アダプタが未実装のため現状未対応で、GitHub URL 依存のみを対象とする (検出されず無言でスキップされる既知の制限。将来対応予定)
- Rust (Cargo) の演算子は `>= 1.2.3` のようにスペースを含む形式も対応し、`>=1.0, <2.0, >=1.0.100` のような3個以上の複数 comparison requirement も Range として解析する。caret/tilde/wildcard と comparator を混在させたカンマ区切りの複数要件 (`^1.2.2, <1.5` / `~1.2, <1.5`) も `semver::VersionReq` で valid 性を確認して Range として検出する (以前は comparator 以外を含むと parse で取りこぼし黙ってスキップしていた)。`<` 上限のない複数下限の混在 (`>=1.2.3, ^1.3`) は下限だけ進めると充足不能になり得るため安全に Skip する
- Cargo.toml の通常依存・inline table・複数行テーブルの更新では TOML の単一引用符 (`'1.0.0'`) も保持する。通常依存・inline table の書き換えは `[dependencies]` / `[dev-dependencies]` / `[build-dependencies]` / `[workspace.dependencies]` / target 固有依存セクションに限定し、`[package.metadata]` 等の依存セクション外の同名キーは書き換えない
- Python の Range 制約は単一セグメントバージョン（例: `>=3,<4`）も正しくパースする。PEP 440 の compatible release 句 `~=1.2` / `~=1.2.3` は明示的な上限を持つレンジ (`~=1.2.3` = `>=1.2.3,<1.3.0`、`~=1.2` = `>=1.2,<2.0`) のため `VersionSpecKind::Range` として扱い、judge で上限を尊重する (`~=1.2.3` は 1.2 系内、`~=1.2` は 1.x 系内に留まり major/minor を跨ぐ誤更新をしない。意味的に同義の `==1.2.*` と整合)。PEP 440 の prefix matching は release segment の `==` / `!=` (`==1.2.*` / `!=1.2.*`) のみ受け付け、`>=1.0.*` / `~=1.0.*` / `==1.0a1.*` / `==1.0.post1.*` / `==1.0+local.*` のような無効形は parse 時点でスキップする。任意一致 `===1.0.*` は prefix matching ではなく固定指定として扱う。書き換えは元のセグメント数を保持する (`~=1.2` → `~=1.9`。`~=1.9.0` にすると上限が `<1.10.0` に変わってしまうため)。`~=1.2, <1.5` のような複合制約は下限側のみ進める。仕様上無効な単一セグメント形式 `~=1` はスキップする。PEP 508 の括弧付き versionspec（例: `requests (>=2.28,<3); python_version < "3.12"`）、空白を含む extras（例: `coverage [toml] >=7,<8`）、末尾カンマ付き version list（例: `paramiko>=3.5,<4,`）も元の形を保って更新する。`pyproject.toml` の Poetry 形式・inline table・PEP 508 配列要素では TOML の単一引用符も保持して更新する
- Poetry の `source = "..."` 付き依存は、`pypi` 以外の source であれば PyPI の候補で誤更新しないよう更新対象から除外する。PEP 621 の `project.dependencies` を `tool.poetry.dependencies` の source 指定で補足している場合も同様に除外する
- uv の `[tool.uv.sources]` で PyPI 以外を指す依存 (`{ workspace = true }` / `git` / `path` / `url` / `index = "<pypi 以外>"`) も同様に更新対象から除外する。workspace メンバーやカスタムインデックス指定を PyPI の同名パッケージ (typosquat を含む) の版で書き換える誤更新を防ぐ。`index = "pypi"` は PyPI そのものなので除外しない
- uv 旧形式の `[tool.uv] dev-dependencies` と PDM の `[tool.pdm.dev-dependencies]` (グループ名 → PEP 508 配列) も解析・更新できる。uv の `dev-dependencies` は `[dependency-groups]` へ移行済みだが既存リポジトリに広く残っている。`[tool.uv]` 内でもキー名の完全一致で判定するため `constraint-dependencies` / `override-dependencies` は書き換えない
- Poetry の多行依存テーブル (`[tool.poetry.dependencies.<name>]` / `[tool.poetry.group.<g>.dependencies.<name>]`) も解析・更新できる。ヘッダ末尾のセグメントが対象パッケージのときだけ配下の `version` 行を書き換える。TOML のクォート付きキー (`"zope.interface"` / `"ruamel.yaml"`) にも対応する — ドットを含む名前は TOML でクォートが必須で、parse (toml クレート) はクォート付きキーも依存として読むため、書き換え側が対応しないと report/apply が矛盾する
- Poetry の演算子なしバージョン (`requests = "2.28.0"`) は完全一致ピン (公式ドキュメントの "Exact requirements"、`==2.28.0` と同義) として `VersionSpecKind::Exact` で解析する。simple 形式・inline table (`{ version = "1.26.0", ... }`) の両方が対象で、明示 `==2.28.0` と同じく依存として surface し (pinned 扱いで `--include-pinned` により更新可能)、更新時は演算子を付けずに新バージョンへ書き換える (`4.2.1` → `5.0.0`)。この bare-exact 解釈は `VersionParser::parse_exact_pin` として Poetry の parse/write 経路 (`parse_poetry_dependency` / `update_poetry_dependency_line`) からのみ呼び、pip / PEP 508 依存指定 (`name==1.2.3` のように演算子必須) では従来どおり通常の `parse` を使うため bare は受理しない。以前は Poetry の bare 版が全 regex を素通りして `None` になり、`--include-pinned` を付けても更新チェック対象にすらならず黙って取りこぼされていた (明示 `==` 版だけが surface する非対称があった)
- `pyproject.toml` の更新は依存配列内に限定する。`[project]` / `[tool.rye]` セクションでは `dependencies` / `dev-dependencies` 配列の中だけを書き換え、`name` / `description` / `keywords` 等のメタデータ文字列が PEP 508 依存指定と一致しても書き換えない (parse が読む範囲と update が書き換える範囲を一致させ、誤書き換えを防ぐ)。`[project.optional-dependencies]` / `[dependency-groups]` はセクション全体が依存配列なので全行が対象。`# "requests>=1.0",` のように行頭・行中の `#` でコメントアウトされた PEP 508 依存指定は parse 側 (TOML パーサ) も無視しているため書き換え対象外とする (parse/write 整合)。行末コメントの右側 (`"requests>=2.0",  # 旧: requests>=1.0`) は左側だけ更新し、コメント内の依存風文字列は触らない。更新走査は TOML のマルチライン文字列 (`"""..."""` / `'''...'''`) の内側を素通しするため、`description` / `readme` 等の docstring に依存配列風テキスト (`dependencies = [ "requests>=2.0" ]`) が含まれていても本物の依存配列と誤認して書き換えない。閉じない擬似配列 (`dependencies = [` だけを含む docstring) で `in_scoped_dep_array` 状態が漏れて後続の `keywords` / `authors` 等のメタデータ配列を破壊することもない (parse は toml クレートで docstring を依存として読まないため、以前は parse/apply が矛盾して静かにメタデータを破壊していた)
- Poetry のマルチプル制約配列形式 (`foo = [{version = "<=1.9", python = ">=3.6,<3.8"}, {version = "^2.0", python = ">=3.8"}]` のように Python バージョン別に異なる制約を配列で指定する形式) は、depup の「1依存=1バージョン=1書き換え」モデルでは配列要素の位置を特定して安全に更新できず、各要素の `python` マーカーごとの `requires_python` 互換性も判定しないため、意図的に更新対象から除外する (誤更新を防ぐ安全側のスキップ)
- Go の `replace` ディレクティブ（単一行・ブロック形式とも）はパースと更新の両方でスキップされる
- Go の `exclude` ディレクティブ（単一行・ブロック形式とも）は依存関係としては surface せず、同じモジュールの更新候補から指定版を除外する。記述自体は書き換えない。引用符付き指定、`require` より前の指定、重複指定も解釈する
- Go Proxy は上流モジュールの生の最新安定版（安定版がなければ最新プレリリース）の `go.mod` を読み、単一版・包含範囲の `retract` を更新候補から除外する。`@v/list` が空なら `@latest` へフォールバックする。`.info` の `Time` は仕様上省略可能なため、欠落または不正値では `UNIX_EPOCH` を使い、age フィルタで公開日不明の版を永久除外しない
- go.mod の `) // comment` のようなコメント付きブロック終端も、`require` / `replace` / `exclude` ブロックの終端として扱う
- go.mod の quoted module path / version（例: `require "golang.org/x/text" "v0.14.0"`）も解析・更新し、引用符を維持する
- Maven Central のクエリはグループID/アーティファクトIDの文字種を検証し、不正な文字によるURLインジェクションを防止する。GitHub Tags (Swift) も owner/repo の文字種を `[A-Za-z0-9._-]` に限定して検証し、`?` / `#` / `..` 等を含む URL によるクエリ汚染・パストラバーサルを防止する (`extract_github_owner_repo` と `validate_package_name` の二層で検証)
- npm/Composer/Cargo は semver の prerelease (`-...`) と build metadata (`+...`) を同時に含むバージョン（例: `^1.2.3-rc.1+build123`）も正しくパースする。ビルドメタデータはバージョン比較時には無視される (semver 仕様)
- Rust (Cargo) の git 依存 (`{ git = "...", branch/tag/rev = "..." }` / 省略形) を検出し、`git ls-remote` でリモート HEAD / タグを取得して更新判定する。branch / 省略形 (デフォルトブランチ) は最新コミットへ更新 (Cargo.toml は書き換えず、Cargo.lock を `--install` の `cargo update` で再解決)、tag は最新 semver タグへ更新 (Cargo.toml の tag 文字列を書き換え)、rev は常にスキップ (pinned 扱い)
- Cargo.lock の git source 末尾 `#<hash>` から現在コミットハッシュを抽出し、`git ls-remote` の結果と比較して差分があれば更新として扱う
- バージョンチェックはマニフェストごとに並列処理される (`futures::stream::buffered`)。並列度は依存数に応じて `clamp(dep_count, 1, 4)` で適応 (最大 4)。内部では各レジストリ別のセマフォ (crates.io は 1、他は 10) が効くため、レート制限は従来どおり尊重される。結果は入力順で返るため表示順は安定する
- `--only` が指定されている場合は `--exclude` より優先される。Cargo リネーム依存では実パッケージ名・マニフェスト名のどちらで指定しても同じ優先順位で判定する
- `.depup` にルートとサブディレクトリが両方含まれる場合、`--install` と Rust の `--age --install` 後処理は、更新されたマニフェストに最も近い（最も深い）対象ディレクトリで実行する
- age が有効な場合 (CLI `--age` / プロジェクト `minimumReleaseAge` / グローバル設定 / 組み込みデフォルト 1w のいずれか) の transitive 依存への適用方法は PM ごとに異なる。install フェーズも judge と同じ解決済み age を使うため、CLI `--age` を明示しなくてもプロジェクト設定やデフォルト 1w が transitive へ反映される:
  - **Rust (cargo)**: `cargo update` 後に Cargo.lock を走査し、age 違反を `cargo update -p <name> --precise <older_version>` で age 内の最新 stable バージョンへ差し戻す (post-install audit)。cargo の再解決で連鎖する新たな違反に備えて最大 5 回まで反復する。resolver 制約違反で差し戻し不可の場合は verbose でスキップ理由を表示して続行
  - **Node.js (pnpm v10.16+)**: `pnpm install` を `npm_config_minimum_release_age=<分>` 環境変数付きで起動する。pnpm は npm 互換の config 規約に従うため、この env var は `.npmrc` の `minimum-release-age=<分>` と等価に解釈される (公式ドキュメント: "This applies to all dependencies, including transitive ones")。pnpm v10.16 未満ではこの env var は未知設定として無視される (graceful no-op)。公式の CLI フラグは現時点で未実装 ([pnpm/pnpm#11224](https://github.com/pnpm/pnpm/issues/11224))
  - **Python (uv)**: `uv sync --exclude-newer <RFC3339>` を注入し uv ネイティブの日時フィルタを利用。transitive 含めて resolve 時に age 制約が効く。さらに `UV_MALWARE_CHECK=1` を常時 env で付与し、`uv sync` / `uv add` などの sync 操作で uv が現在 locked された resolution を OSV の MAL advisories と照合する preview マルウェアチェックを有効化する (`uv audit` コマンドとは別の、同時発表された独立 preview 機能。公式: https://astral.sh/blog/uv-audit)。マルウェアにマッチした場合は uv 側で sync が中止され、悪意あるコードの実行前に止まる。preview 機能のため将来挙動が変わる可能性はあるが、機能未対応の古い uv バージョンではこの env var は通常無視されるため、強制 ON でも既存環境のビルドは壊さない方針
  - **その他 (npm/yarn/bun/pip/poetry/rye/pipenv/bundle/composer/gradle/swift/go)**: transitive 依存へのネイティブ age サポートが無いため direct deps のみ age 制御される。verbose モードで通知
- pnpm の fallback には既知の不具合あり: 同一 major 内の intermediate 版への fallback が失敗するケース ([pnpm/pnpm#11203](https://github.com/pnpm/pnpm/issues/11203))、`minimumReleaseAgeExclude` 除外依存の transitive が age 違反のとき `ERR_PNPM_NO_MATURE_MATCHING_VERSION` で失敗するケース ([pnpm/pnpm#11068](https://github.com/pnpm/pnpm/issues/11068)) など。transitive が基本的には守られるが、完全ではない点に注意
- `--age` のデフォルトは `1w` (組み込みデフォルト)。グローバル設定 `~/.config/depup/config.toml` の `age = "1w"` 等で上書き可能。優先順位は `--no-age` > `--age` > グローバル設定 > 組み込みデフォルト (1w)。`--age` と `--no-age` は同時指定できない (clap の conflicts_with)
- グローバル設定ファイル `~/.config/depup/config.toml` は初回実行時に自動生成される (claw-hooks 方式)。雛形は組み込みデフォルトと一致するキーをコメント付きで書き出す (`age = "1w"`、`osv` はコメントアウト)。生成失敗時 (権限など) は警告を出して処理を継続し、組み込みデフォルトで動作する。既存ファイルは絶対に上書きしない
- `--max-change <LEVEL>` (patch / minor / major) で許容する bump レベルを制限。現在版と候補版を semver の数値コアで比較し、`Patch < Minor < Major` の順序で `level <= max` を通す。除外された候補がある場合は `SkipReason::ChangeLevelLimited(level)` で skip。グローバル設定 `max_change = "minor"` で常時設定可能。優先順位は CLI > config > 組み込みデフォルト (制限なし)。テキスト出力の major/minor/patch 表示ラベル (`VersionChangeType`) も同じ `ChangeLevel::from_versions` (任意桁10進の数値コア比較) に委譲しており、judge の分類と表示が食い違わない (u64 超の版や大文字 V 接頭辞でも一致)
- age 解決の優先順位 (build_filter で確定): プロジェクト `minimumReleaseAge` (pnpm / bun) > CLI `--age` > `--no-age` > グローバル設定 `age` > 組み込みデフォルト 1w。`minimumReleaseAge` はプロジェクトポリシーとして CLI を上書きする。CLI が無視される場合は `⚠ --age ignored: project's minimumReleaseAge (N days from <source>) takes precedence` を黄色で stderr に通知。pnpm と bun の両方が値を持つ場合はより厳しい (max) を採用。bun は `bunfig.toml` の `[install] minimumReleaseAge` (秒単位の整数)。`main.rs` では age の resolve はせず、judge フェーズは `build_filter` で、install フェーズ (PM install / Rust lock audit) は `orchestrator.resolved_min_age()` で解決する。両者は同じ `resolve_age` ロジックを共有するため、CLI `--age` 未指定でもプロジェクト `minimumReleaseAge` / グローバル設定 / 組み込みデフォルト 1w が install 後の transitive 依存へ一貫して反映される (Rust の post-install audit は更新があった manifest のディレクトリに限定して実行)
- `--osv` で OSV.dev API による脆弱性チェックを有効化。`judge_with_osv` は採用しようとした候補だけを `https://api.osv.dev/v1/query` に POST し、`vulns` が空でなければ その version を candidate から除外して再 judge → 次に古い候補へフォールバックする。全 candidate を網羅的にチェックする方式は採らない (1000+ バージョンを持つ `@angular/*` 等で実用速度を確保するため)。通常 1 依存あたり 1〜2 API call で完了する。API エラー時は元の候補を採用して `--verbose` で警告。Swift は OSV ecosystem 未対応のためスキップ。グローバル設定 `osv = true` で常時有効化可能。優先順位は `--no-osv` > `--osv` > グローバル設定 > 組み込みデフォルト (false)。OSV API は認証トークン不要。脆弱版除外は言語別の `compare_dependency_versions` を使うため、Python の PEP 440 ローカルバージョン (`1.0+cu121` 等) が build metadata を無視する semver 比較で誤って同列と判定され、安全な候補まで NoSuitableVersion に落ちることがない
- OSV フォールバックの警告 (`X vulnerable, falling back`) は設計どおりの正常動作の通知であり、exit code には影響しない (exit code 2 は OSV 警告以外のエラーがある場合のみ)
- npm alias 依存 (`"react": "npm:@preact/compat@^17"`) は実パッケージ名 (`@preact/compat`) でレジストリ照会し、書き戻しには JSON キー (alias 名) を使う (Cargo の rename 依存と同じ name / manifest_name パターン)。alias 接頭辞 `npm:<real>@` は更新後も保持される
- Bun Catalogs は root `package.json` のトップレベル `catalog` / `catalogs` と `workspaces.catalog` / `workspaces.catalogs` を解析・更新する。workspace package 側の `catalog:` / `catalog:<name>` 参照は、参照先 catalog 定義で一元管理されるため書き換えず保持する。複数の catalog object を更新するときは、JSON 内の出現順に範囲をソートしてから後方順に置換し、先行置換による byte offset のずれで後続 catalog を壊さない。pnpm の `pnpm-workspace.yaml` catalogs は現時点では未対応で、`package.json` 内の `catalog:` 参照は安全側でスキップする
- 同じマニフェストキーの依存が複数箇所に宣言されている場合 (Cargo.toml の `[dependencies]` + `[dev-dependencies]`、package.json の dependencies + devDependencies、pyproject の main + dev group、Gemfile の通常宣言 + `group :test` 等)、名前だけでは更新位置を一意に決められないため writer は曖昧な更新を拒否する。複数の Gradle 依存が同じバージョン変数または version catalog の `version.ref` を共有する場合も last-write-wins を避けて拒否し、スキップ・固定された別宣言を暗黙に書き換えない
- Cargo.toml の複数行テーブル (`[dependencies.<pkg>]`) はセクション追跡の行ベースで更新され、`features = [...]` が `version` より前にあるキー順でも、テーブル内コメントに `version = "..."` 文字列があっても正しく動く。`update_git_tag` は inline table と複数行 `tag` の両方で同方式を使い、依存/patch セクション外の同名キーを触らず、単一引用符も保持する
- Package.swift の URL マッチは末尾境界付き (`grpc/grpc-swift` の更新が `grpc/grpc-swift-nio` の宣言に前方一致しない)
- Composer の platform package 判定はベンダーレス名のみ対象 (`php-amqplib/php-amqplib` のような `/` を含む実在パッケージは除外されない)。stability flag (`^1.0@dev` / `~1.2.3@beta` / `1.2.3@RC`) は `raw` と `suffix` に保持され、更新後も維持される (`^1.5.0@dev`)。制約本体の解釈はフラグを外した文字列で行うが、フラグを捨てると `--diff` の before/after が実ファイルと食い違う (表示は `^1.0` → `^1.5.0`、実際の書き込みは `^1.0@dev` → `^1.5.0@dev`)。インラインエイリアス (`1.0.0 as 1.1.0` / `1.0.0@dev as 1.1.0`) は別バージョンへのエイリアス宣言のため、レジストリ最新版で上書きすると宣言が壊れる。更新対象から除外する (安全側のスキップ)
- Gemfile はクォートを考慮して行コメント (`# ...`) を除去してから判定するため、`gem 'debug' # things to do` のような行末コメントの "do" をブロック開始と誤認しない。コメント中の `git:` 等の文言でレジストリ外依存と誤判定されない。バージョンなし gem への挿入は `groups:` / `install_if:` / `force_ruby_platform:` オプション付き行にも対応。CRLF の Gemfile は改行コードを保持して更新される。バージョンなし gem + 行末条件修飾子 (`gem 'wdm' if Gem.win_platform?` / `unless ...`、括弧付き `gem('wdm') if ...`) も version 挿入対象とし修飾子を保持する (以前は parse が versionless=更新可能として拾うのに update 側の挿入パターンが `if`/`unless` を許容せず report/apply が矛盾していた。versioned + 修飾子は既に対応済み)
- Gradle のコメント除去は文字列リテラルを認識する (`exclude 'META-INF/*.kotlin_module'` の `/*` や `url 'https://...'` の `//` をコメント扱いしない)。宣言全体が `/* */` でコメントアウトされた依存・rich version ブロック・変数定義は parse / update とも無視される。変数定義の更新はバージョン値のみ置換するためインデントと行末コメントが保持される。Kotlin DSL の型注釈付き変数 (`val x: String = "..."`)、`const val x = "..."` (Kotlin DSL で最も一般的なバージョン定義形式)、Groovy の型宣言変数 (`String x = "..."`、`def` を伴わない明示型) も変数定義として解決する。`maven { url 'http://host:8081' }` のような URL は依存として誤検出されない。depup が追跡しない場所 (`gradle.properties` / `val x by extra("...")` / 計算値など) で定義され解決できない変数を参照する文字列記法依存は、空の `Any` 依存を作らず依存ごとスキップする (空の `Any` を作ると judge が「更新あり」と報告した後に writer が書き換え先を見つけられず失敗し、報告と適用が矛盾するため)
- Range の上限抽出は Maven 形式 (完全アンカー) を最優先で評価し、ハイフンレンジは npm/Composer 仕様どおり前後スペース必須 (`[1.0-2,2.0)` の qualifier 付き下限に誤マッチしない)。`<` / `<=` が複数並ぶ場合 (例: `>=1,<2,<=3`) は最も厳しい上限を採用する
- npm/Composer のハイフンレンジ (`A - B`) は両端が裸の version (partial 含む) でなければ parse 時点で None で弾く。`^1.0 - 2.0` / `~1.0 - 2.0` / `>=1.0 - 2.0` のような演算子付き端点は node-semver / composer/semver 仕様上 invalid で、過受理すると `format_updated` が `^X.Y.Z - 2.0` のような構文エラーになる制約を書き戻してしまうため (`npm install` / `composer install` が失敗する)。Node では `>=1.0 - 2.0` のような形式が comparator set 判定で誤受理されないよう、parse 冒頭で「` - ` を含むが両端が裸 version になっていない」入力を明示的に拒否する
- 書き換え結果が現在の raw 表記と同一になる場合 (例: ワイルドカード `1.x` の範囲内に最新版がある場合) は Update ではなく AlreadyLatest として扱う (毎回 phantom update が報告される問題の防止)。`1.x` → `2.x` のように形が変わる更新は従来どおり Update
- `--max-change` で候補が空になったとき、`ChangeLevelLimited` を返すのは「現在版より新しい候補が max-change で除外された」場合のみ。新しい候補がそもそも無ければ `AlreadyLatest`
- レジストリ層の一貫性: PyPI は yanked リリース (全ファイル yanked または ファイル 0 件) を候補から除外。Packagist の `time` 欠損は `UNIX_EPOCH` フォールバック (age フィルタで永久除外されない)。npm の dist-tags.latest 超のバージョンは prerelease と判定できるもののみ候補に残す (canary/beta 利用者の更新を妨げない)。GitHub Tags は Link ヘッダの `rel="next"` を辿って全ページ取得 (最大10ページ)、403 + `X-RateLimit-Remaining: 0` はレート制限として報告。RubyGems は `platform != "ruby"` のエントリを除外。Go proxy の `.info` / `.mod` URL はバージョン側も case-encode (Go 仕様どおり ASCII 大文字のみ対象、`v1.0.0-RC1` → `v1.0.0-!r!c1`)。HTTP クライアントは 5xx もリトライし `Retry-After` を尊重 (上限10秒)
- `git ls-remote` は `GIT_TERMINAL_PROMPT=0` + 30秒タイムアウト付きで実行され、認証プロンプトでハングしない。URL スキームは許可リスト (https/http/ssh/git/git@/file) で検証され、`ext::` 等は拒否される
- `git ls-remote` の取得結果は URL ごとのロックで single-flight 化し、同じ URL への並行要求を1回のプロセス起動へ集約する。異なる URL は並行実行を維持する
- git 依存の URL に埋め込まれたアクセストークン (`https://x-access-token:<TOKEN>@github.com/org/private.git`) は、エラー・スキップ理由として表示される経路で userinfo を `***` に伏せる。fetch 失敗時のスキップ理由はテキスト出力・JSON 出力の両方に出るため、生の URL をエラーへ保持すると CI ログや成果物にトークンが残る。git が stderr へ URL をエコーする場合に備えて出力側も伏せる。キャッシュキーには生 URL を使い続ける (`git@host:path` 形式のユーザ名はトークンではないので保持する)
- PM 検出: Bun はテキスト形式 `bun.lock` (Bun 1.2+) と旧 `bun.lockb` の両方を検出。Rye は `requirements.lock` / `requirements-dev.lock` で検出 (`rye.lock` というファイルは存在しない)
- Cargo workspace の glob 形式 members (`members = ["crates/*"]`、末尾セグメントの `util-*` も可) を展開し、`[workspace] exclude` を除外する。pnpm-workspace.yaml の packages は block-style (`- 'packages/*'`) に加えて flow-style 配列 (`packages: ['packages/*', 'apps/*']`、`]` が次行以降にある複数行形式も可) を解析し、いずれもインラインコメント付き行 (`- 'packages/*' # apps`) と否定パターン (`- '!packages/legacy'` / flow 内の `'!packages/legacy'`) に対応。検出されたマニフェストは重複排除される (`.depup` にルートとサブディレクトリを併記しても二重処理されない)
- pnpm の minimumReleaseAge は `.npmrc` の分単位数値 (`minimum-release-age=14400`) と `=` 前後の空白、pnpm-workspace.yaml のインラインコメント付き値、package.json の数値型も読める。age 通知の `<source>` は実際に値が読まれたファイル名を表示する
- マニフェストの書き込みは同一ディレクトリの一時ファイル + rename によるアトミック置換 (途中失敗で部分内容が残らない)。rename の前に一時ファイルを `sync_all` で永続化し、rename 後はディレクトリエントリも best-effort で fsync する — `fs::write` はページキャッシュへ書くだけなので、fsync しないと rename のメタデータだけが先に永続化され、電源断でゼロ長または部分内容のマニフェストが残りうる (元の内容も失われる)。既存ファイルのパーミッションは引き継ぎ、読み取り専用ファイルへの書き込みは従来どおりエラー (書き込み可否は rename 前に `OpenOptions::write(true).open` で明示的に検査する)。マニフェスト自体が symlink の場合は `canonicalize` でリンク先の実体へ解決し、実体側のディレクトリで一時ファイル + rename を行う (rename(2) は symlink 自体を置換してしまうため。これによりリンク先 (モノレポの共有マニフェスト等) を更新し symlink 構造を保つ。tmp も実体側に作るため同一ファイルシステム内に収まり EXDEV にならない)
- `--diff` は実際にマニフェストへ書き込まれる更新のみ表示する (branch / rev / デフォルトブランチの git 依存は Cargo.lock 側の更新のみなので diff に出さない。tag の git 依存は表示)
- Cargo.lock はマニフェストのディレクトリから実行ルートまで上方向に探索される (virtual workspace のメンバーでも git 依存の current_commit 取得と post-install age 監査が機能する)。同名 git 依存が複数 URL から lock されている場合は URL で対応付ける。age 監査の差し戻しは `cargo update -p <name>@<current> --precise` の完全修飾 spec を使う (同名複数バージョン lock でも ambiguous エラーにならない)
- Tauri バージョン同期は judge の明示的フィルタ結果 (`--exclude` / `--only` / 言語フィルタ / pinned / `--max-change` / fetch・parse 失敗による Skip) を上書きしない (上書きするのは AlreadyLatest / NoSuitableVersion のみ)。同期先候補には解決済み age を適用し、npm パッケージごとに実在するバージョンを選ぶ (`@tauri-apps/api` と `@tauri-apps/cli` のパッチ集合差に対応)
- git tag 依存の「最新 semver タグ」選定は semver 形状 (`v1.2.3` / `1.2`、数値コア 2〜3 セグメント) のタグのみ対象 (日付タグ `20240601-hotfix` 等が選ばれない)。git tag の更新はレジストリ依存と同じく `--max-change` を尊重し、上限を超える bump は `ChangeLevelLimited` でスキップする
- `--age` / グローバル設定の duration パース (`213503982334602d` 等の巨大値) はオーバーフローせずエラーを返す
- `--install` で起動するパッケージマネージャは `which` クレートで PATH 解決してから `Command::new` に渡す。Windows では `npm` / `pnpm` / `yarn` / `bun` / `composer` / `gradle` 等が `.exe` ではなく `.cmd` / `.bat` シムで配布されることが多く、`Command::new("pnpm")` だと `CreateProcessW` が拡張子を `.exe` で解決して `program not found` で失敗していた (Issue #1)。`which` は `PATHEXT` を考慮して `.cmd` / `.bat` の実体パスを返すため、フルパスを `Command::new` に渡せばシム経由でも起動できる。`.cmd` / `.bat` の実体パスを `Command::new` に直接渡すと Rust 標準ライブラリ (1.77+) の引数自動エスケープが効くため、CVE-2024-24576 (BatBadBut) の影響も同時に緩和される。解決失敗時は元の program 名をフォールバックで返し、後段の `Command::new` で従来通りの `program not found` 系エラーを返す。`./gradlew` のようにセパレータを含む名前は `which_in` で **install 実行ディレクトリ基準**に解決する — `which::which` はプロセスの CWD 基準で絶対化するため、モノレポでルートから `depup --install` するとサブプロジェクトの wrapper ではなくルートの `gradlew` (別の Gradle ディストリビューション) を起動していた
- `--install` の PHP 処理は `composer install` ではなく `composer update` を使う。`composer install` は既存の `composer.lock` を再利用するため、depup が直前に変更した `composer.json` の制約を lock へ反映できず、旧版を入れるか root 要件不一致で失敗する。`composer update` で制約を再解決して lock を更新する
- `--age` を transitive 依存へネイティブに適用できる言語かどうかは `Language::has_native_transitive_age_support()` が単一の情報源 (Node = pnpm の env var / Python = uv の `--exclude-newer` / Rust = post-install lock 監査)。`match` の網羅性検査が効くため、言語を追加したときに `--verbose` の通知漏れが起きない
