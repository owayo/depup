# Official Version Specification Reference

各言語のパッケージマネージャが公式にサポートするバージョン指定構文の網羅リスト。
`depup 分類` は `src/parser/*` と `src/manifest/*` の現実装に基づく。

## Table of Contents

- [Node.js (npm/yarn/pnpm)](#nodejs-npmyarnpnpm)
- [Python (PEP 440/Poetry)](#python-pep-440poetry)
- [Rust (Cargo)](#rust-cargo)
- [Go (go.mod)](#go-gomod)
- [Ruby (RubyGems/Bundler)](#ruby-rubygemsbundler)
- [PHP (Composer)](#php-composer)
- [Java (Gradle)](#java-gradle)
- [Swift (SPM)](#swift-spm)
- [Range 上限抽出（UpdateJudge）](#range-上限抽出updatejudge)

---

## Node.js (npm/yarn/pnpm)

**公式仕様**: [node-semver](https://github.com/npm/node-semver)

| 構文 | 例 | 説明 | depup 分類 |
|------|---|------|-----------|
| Exact | `1.2.3` | 完全一致 | Exact |
| Equal prefix | `=1.2.3` | 完全一致（明示） | Exact |
| v-prefix | `v1.2.3` | v 接頭辞付き | Exact |
| Caret | `^1.2.3` | 左端の非ゼロを保持 | Caret |
| Caret partial | `^1.2`, `^1` | 部分バージョン caret | Caret |
| Tilde | `~1.2.3` | マイナー固定 | Tilde |
| Tilde partial | `~1.2`, `~1` | 部分バージョン tilde | Tilde |
| Greater or equal | `>=1.2.3` | 以上 | GreaterOrEqual |
| Greater | `>1.2.3` | 超過 | Greater |
| Less or equal | `<=1.2.3` | 以下 | LessOrEqual |
| Less | `<1.2.3` | 未満 | Less |
| Wildcard | `*`, `1.x`, `1.2.*`, `1.X` | ワイルドカード | Wildcard |
| Hyphen range | `1.0.0 - 2.0.0` | ハイフン範囲 | Range |
| OR range | `^1 \|\| ^2` | OR 結合 | Range |
| Comparator set | `>=1.0.0 <2.0.0` | AND 結合 | Range |
| Partial range | `1.2` | 部分バージョン（暗黙range） | Range |
| Dist-tag | `latest`, `next`, `canary` | タグ | スキップ（浮動参照） |
| npm alias | `npm:@scope/pkg@^1.0` | npm エイリアス（右側制約を解釈） | 内部制約に準拠 |
| Protocol | `workspace:*`, `file:`, `git://`, `github:`, `https://` | プロトコル参照 | スキップ |

### node-semver の特殊ケース

- `^0.0.3` = `>=0.0.3 <0.0.4` (パッチ固定)
- `^0.0` = `>=0.0.0 <0.1.0`
- `^0` = `>=0.0.0 <1.0.0`
- `~0.0.3` = `>=0.0.3 <0.1.0`
- Build metadata (`+build`) は比較時に無視
- ハイフン範囲の部分バージョン: `1.2.3 - 2.3` → `>=1.2.3 <2.4.0-0`

### Node の実装注記

- `npm:` alias は `npm:<name>@` プレフィックスを保持したまま更新する
- `workspace:` / `file:` / `link:` / `git+` / `git://` / `github:` / `http(s)://` などは更新対象外
- 空文字列 `""` や `git+https://...#semver:^1.0.0` は現在の depup ではパース対象外

---

## Python (PEP 440/Poetry)

**公式仕様**: [PEP 440](https://peps.python.org/pep-0440/), [PEP 508](https://peps.python.org/pep-0508/), [Poetry docs](https://python-poetry.org/docs/dependency-specification/)

| 構文 | 例 | 説明 | depup 分類 |
|------|---|------|-----------|
| Exact (PEP 440) | `==1.2.3` | 完全一致 | Exact |
| Exact wildcard | `==1.2.*` | メジャー.マイナー一致 | Range |
| Arbitrary equality | `===v1.2-custom` | arbitrary equality (`===`) | Exact（比較時も識別子を保持） |
| Compatible release | `~=1.2.3` | `>=1.2.3, <1.3.0` 相当 | Range |
| Compatible release (2-part) | `~=1.2` | `>=1.2, <2.0` 相当 | Range |
| Greater or equal | `>=1.2.3` | 以上 | GreaterOrEqual |
| Greater | `>1.2.3` | 超過 | Greater |
| Less or equal | `<=1.2.3` | 以下 | LessOrEqual |
| Less | `<1.2.3` | 未満 | Less |
| Not equal | `!=1.2.3` | 除外 | Range |
| Not equal wildcard | `!=1.2.*` | メジャー.マイナー除外 | Range |
| Wildcard | `*`, `1.*` | 任意バージョン | Wildcard |
| Range (comma) | `>=1.0,<2.0` | AND 結合 | Range |
| Epoch | `>=1!2.0` | エポック接頭辞 | 演算子付きで対応（比較時も保持） |
| Caret (Poetry) | `^1.2.3` | Poetry 互換（caret） | Caret |
| Tilde (Poetry) | `~1.2.3` | Poetry 互換（tilde） | Tilde |
| Environment markers | `; python_version>="3.8"` | 環境マーカー | パース後除去 |
| Extras | `package[extra]>=1.0` | extras 指定 | extras保持 |
| Local version | `==1.0+local1` | ローカル版 | Range（local を保持し、自動書き換えはスキップ） |
| Pre-release | `==1.0a1`, `==1.0b1`, `==1.0rc1` | PEP 440 プレリリース | Exact（比較時も識別子を保持） |
| Post-release | `==1.0.post1` | ポストリリース | Exact（比較時も識別子を保持） |
| Dev release | `==1.0.dev1` | 開発リリース | Exact（比較時も識別子を保持） |

### PEP 440 の特殊ケース

- `~=1.2.3` は `>=1.2.3, <1.3.0` と同値
- `~=1.2` は `>=1.2, <2.0` と同値
- エポック (`1!`) はバージョン順序のオーバーライド
- `==1.2.*` は `>=1.2.0, <1.3.0` と同値
- depup は演算子付き specifier を対象にするため、`1.0+local1` のような単体値は対象外

---

## Rust (Cargo)

**公式仕様**: [Cargo Reference - Specifying Dependencies](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html)

| 構文 | 例 | 説明 | depup 分類 |
|------|---|------|-----------|
| Caret (implicit) | `1.2.3` | デフォルト（^相当） | Caret |
| Caret (explicit) | `^1.2.3` | 左端の非ゼロを保持 | Caret |
| Caret partial | `^1.2`, `^1`, `^0.0` | 部分 caret | Caret |
| Tilde | `~1.2.3` | マイナー固定 | Tilde |
| Tilde partial | `~1.2`, `~1` | 部分 tilde | Tilde |
| Exact | `=1.2.3` | 完全一致 | Exact |
| Greater or equal | `>=1.2.3` | 以上 | GreaterOrEqual |
| Greater | `>1.2.3` | 超過 | Greater |
| Less or equal | `<=1.2.3` | 以下 | LessOrEqual |
| Less | `<1.2.3` | 未満 | Less |
| Wildcard | `*`, `1.*`, `1.2.*` | ワイルドカード | Wildcard |
| Multiple requirements | `>=1.2, <1.5` | AND 結合 | Range |
| Bare partial | `1.2`, `1` | 部分バージョン（caret相当） | Caret |
| Pre-release | `1.0.0-alpha` | プレリリース | Caret/Exact（prefix次第） |
| Build metadata | `1.0.0+build` | ビルドメタデータ | 対応（比較時は semver 仕様どおり無視） |
| Dotted key | `tokio.version = "1.38"` | TOML dotted key 形式 | 対応（依存セクション限定） |
| Path 依存 | `{ path = "../x", version = "0.1" }` | ローカル path 依存 | スキップ（`version` 併記でも） |

### Cargo の特殊ケース

- `^0.0.3` = `>=0.0.3, <0.0.4`
- `^0.0` = `>=0.0.0, <0.1.0`
- `^0` = `>=0.0.0, <1.0.0`
- `~1.2.3` = `>=1.2.3, <1.3.0`
- `~1.2` = `>=1.2.0, <1.3.0`
- `~1` = `>=1.0.0, <2.0.0`
- `*` = `>=0.0.0`
- `1.*` = `>=1.0.0, <2.0.0`

---

## Go (go.mod)

**公式仕様**: [Go Module Reference](https://go.dev/ref/mod)

| 構文 | 例 | 説明 | depup 分類 |
|------|---|------|-----------|
| Semver | `v1.2.3` | 標準 semver（v接頭辞必須） | Exact |
| Pre-release | `v1.2.3-beta.1` | プレリリース | Exact |
| Pseudo-version | `v0.0.0-20210101120000-abcdef123456` | コミットベース | Exact |
| Extended pseudo | `v1.2.4-0.20210101120000-abcdef123456` | 拡張 pseudo | Exact |
| Extended pseudo (prerelease) | `v1.2.4-beta.0.20210101120000-abcdef123456` | プレリリース付き拡張 pseudo | Exact |
| +incompatible | `v2.0.0+incompatible` | v2+ 互換性タグ | Exact (suffix) |
| `// pinned` comment | `v1.2.3 // pinned` | ピン留めコメント | マニフェストレベル |
| `// indirect` comment | `v1.2.3 // indirect` | 間接依存 | dev扱い |

### Go の特性

- go.mod ではバージョン範囲指定が存在しない（常にピン留め）
- Minimal Version Selection (MVS) アルゴリズムでバージョン解決
- `replace` ディレクティブはスキップ
- `exclude` ディレクティブ自体は書き換えず、指定版を更新候補から除外する
- `retract` は情報源の `go.mod` から読み、単一版・包含範囲を更新候補から除外する

### pseudo-version の3形式

- `vX.0.0-yyyymmddhhmmss-abcdefabcdef` - タグなし
- `vX.Y.Z-pre.0.yyyymmddhhmmss-abcdefabcdef` - プレリリースタグ後のコミット
- `vX.Y.(Z+1)-0.yyyymmddhhmmss-abcdefabcdef` - リリースタグ後のコミット

---

## Ruby (RubyGems/Bundler)

**公式仕様**: [RubyGems Specification](https://guides.rubygems.org/specification-reference/), [Bundler docs](https://bundler.io/gemfile.html)

| 構文 | 例 | 説明 | depup 分類 |
|------|---|------|-----------|
| Exact (bare) | `1.2.3` | 完全一致 | Exact |
| Exact (= prefix) | `= 1.2.3` | 完全一致（明示） | Exact |
| Pessimistic | `~> 1.2.3` | `>= 1.2.3, < 1.3.0` | Tilde |
| Pessimistic (minor) | `~> 1.2` | `>= 1.2, < 2.0` | Tilde |
| Greater or equal | `>= 1.2.3` | 以上 | GreaterOrEqual |
| Greater | `> 1.2.3` | 超過 | Greater |
| Less or equal | `<= 1.2.3` | 以下 | LessOrEqual |
| Less | `< 1.2.3` | 未満 | Less |
| Not equal | `!= 1.2.3` | 除外 | Range |
| Compound (comma) | `>= 1.0, < 2.0` | AND（カンマ区切り） | Range |
| Compound (space) | `>= 1.0 < 2.0` | AND（スペース区切り） | Range |
| No version | `gem 'rails'` | バージョン指定なし | Any |
| 4-segment | `1.2.3.4` | 4セグメント版 | Exact |
| Pre-release | `1.2.3.pre` | プレリリース | Exact |
| Source | `:git`, `:github`, `:path` | ソース指定 | スキップ |

### Ruby の特殊ケース

- `~> 2.0.0` = `>= 2.0.0, < 2.1.0`
- `~> 2.0` = `>= 2.0, < 3.0`
- `~> 0` = `>= 0, < 1`（あまり使われない）
- 4セグメント版（`1.2.3.4`）も有効

---

## PHP (Composer)

**公式仕様**: [Composer - Version Constraints](https://getcomposer.org/doc/articles/versions.md)

| 構文 | 例 | 説明 | depup 分類 |
|------|---|------|-----------|
| Exact | `1.2.3` | 完全一致 | Exact |
| Caret | `^1.2.3` | 左端の非ゼロを保持 | Caret |
| Caret partial | `^1.2`, `^1` | 部分 caret | Caret |
| Tilde | `~1.2.3` | マイナー固定 | Tilde |
| Tilde partial | `~1.2` | `>=1.2.0, <2.0.0` | Tilde |
| Greater or equal | `>=1.2.3` | 以上 | GreaterOrEqual |
| Greater | `>1.2.3` | 超過 | Greater |
| Less or equal | `<=1.2.3` | 以下 | LessOrEqual |
| Less | `<1.2.3` | 未満 | Less |
| Not equal | `!=1.2.3` | 除外 | Range |
| Wildcard | `1.2.*`, `1.*`, `*` | ワイルドカード | Wildcard |
| x notation | `1.x`, `1.2.x` | x ワイルドカード | Wildcard |
| OR (double pipe) | `^1 \|\| ^2` | OR 結合 | Range |
| OR (single pipe) | `^1 \| ^2` | OR 結合（非推奨） | Range |
| AND (space) | `>=1.0 <2.0` | AND 結合 | Range |
| AND (comma) | `>=1.0,<2.0` | AND 結合 | Range |
| Hyphen range | `1.0 - 2.0` | ハイフン範囲 | Range |
| Stability flag | `1.0@dev` | 安定性フラグ | 本体と分けて保持し、更新後も再付与 |
| v-prefix | `v1.2.3` | v 接頭辞 | Exact |
| Platform | `php`, `ext-*`, `lib-*` | プラットフォーム | スキップ |

### Composer の特殊ケース

- `~1.2` = `>=1.2.0, <2.0.0`（npm の `~` とは異なる）
- `~1.2.3` = `>=1.2.3, <1.3.0`
- `^0.3` = `>=0.3.0, <0.4.0`
- 安定性フラグ (`@dev`, `@alpha` 等) はバージョン制約とは別

### Composer のブランチ・開発版参照

- `dev-main` - ブランチ名による参照
- `1.x-dev` - バージョン風ブランチ名（`.x-dev` サフィックス必須）
- `dev-main#abc1234` - コミットハッシュ参照
- `1.0.0 as 1.1.0` - インラインエイリアス

---

## Java (Gradle)

**公式仕様**: [Gradle - Dependency Versions](https://docs.gradle.org/current/userguide/dependency_versions.html)

| 構文 | 例 | 説明 | depup 分類 |
|------|---|------|-----------|
| Exact | `1.2.3` | 完全一致 | Exact |
| Exact with suffix | `1.2.3-SNAPSHOT`, `1.2.3.RELEASE` | サフィックス付き | Exact |
| Prefix version | `1.2.+` | プレフィックスマッチ | Wildcard |
| Dynamic latest | `latest.release` | 最新リリース | Wildcard |
| Dynamic integration | `latest.integration` | 最新（SNAPSHOT含む） | Wildcard |
| Maven closed range | `[1.0, 2.0]` | 閉区間 | Range |
| Maven half-open | `[1.0, 2.0)` | 半開区間 | Range |
| Maven lower-open | `(1.0, 2.0]` | 下限開放 | Range |
| Maven open-upper | `[1.0, )` | 上限なし | Range |
| Maven open-lower | `(, 2.0]` | 下限なし | Range |
| Strict version | `1.2.3!!` | 厳密バージョン（サフィックス形式） | Exact (suffix `!!` 保持) |
| Reject version | `1.2.+` reject `1.2.5` | 拒否指定 | ー |
| Prefer version | `1.+` prefer `1.2.3` | 優先指定 | ー |
| Version catalog | `libs.xxx` | バージョンカタログ | `gradle/*.versions.toml` 側で更新 |
| Platform | `platform()`, `enforcedPlatform()` | BOM/Platform | 対応（ラッパを剥がして解析） |
| Test fixtures | `testFixtures()` | テストフィクスチャ変種 | 対応（ラッパを剥がして解析） |
| ext ドット代入 | `ext.ver = '1.0'` | ext ブロック外の変数定義 | 変数展開 |
| 修飾付き参照 | `"g:n:${Versions.ver}"` | オブジェクト経由の参照 | 変数展開（曖昧な短名はスキップ） |
| Variable (Groovy) | `def ver = '1.0'` | Groovy 変数 | 変数展開 |
| Variable (Kotlin) | `val ver = "1.0"` | Kotlin 変数 | 変数展開 |
| ext block | `ext { ver = '1.0' }` | ext ブロック変数 | 変数展開 |
| Interpolation | `"group:name:$ver"` | 文字列補間 | 変数展開 |

### Gradle の特殊ケース

- Maven スタイル範囲: `[` = inclusive, `(` = exclusive
- 代替記法: `]1.0, 2.0[` = `(1.0, 2.0)` (`]` = exclusive lower, `[` = exclusive upper)
- `!!` は Gradle 7+ の strict version constraint
- `strictly("1.5")` / `require("1.5")` / `prefer("1.7")` / `reject("1.4")` は rich version 構文
- `[1.7, 1.8[!!1.7.25` - strictly + prefer の短縮記法
- Version Catalog (`libs.xxx`) は TOML ベースの別ファイル管理
- `+` プレフィックスマッチ: `1.2.+` は `1.2.0` 〜 `1.2.99...` にマッチ
- `+` 単独 = 全バージョンにマッチ
- depup の parser は `Exact` / `prefix (+)` / `latest.*` / Maven range を主対象とする

---

## Swift (SPM)

**公式仕様**: [Swift Package Manager](https://docs.swift.org/package-manager/PackageDescription/PackageDescription.html)

| 構文 | 例 | 説明 | depup 分類 |
|------|---|------|-----------|
| from: | `.package(url:, from: "1.0.0")` | `>= 1.0.0, < 次のメジャー` | Caret |
| .upToNextMajor | `.package(url:, .upToNextMajor(from: "1.0.0"))` | from: と同じ | Caret |
| .upToNextMinor | `.package(url:, .upToNextMinor(from: "1.0.0"))` | `>= 1.0.0, < 1.1.0` | Tilde |
| exact: (keyword) | `.package(url:, exact: "1.0.0")` | 完全一致 | Exact |
| .exact() (method) | `.package(url:, .exact("1.0.0"))` | 完全一致 | Exact |
| Half-open range | `.package(url:, "1.0.0"..<"2.0.0")` | 半開区間 | Range |
| Closed range | `.package(url:, "1.0.0"..."2.0.0")` | 閉区間 | Range |
| branch: | `.package(url:, branch: "main")` | ブランチ指定 | スキップ |
| revision: | `.package(url:, revision: "abc123")` | リビジョン指定 | スキップ |
| .branch() | `.package(url:, .branch("main"))` | ブランチ（メソッド） | スキップ |
| .revision() | `.package(url:, .revision("abc123"))` | リビジョン（メソッド） | スキップ |
| path: | `.package(path: "../local")` | ローカルパス | スキップ |
| name: parameter | `.package(name:, url:, from:)` | 名前付き（5.2-5.5） | 対応 |

### Swift の特殊ケース

- `from: "1.0.0"` は `"1.0.0"..<"2.0.0"` と同値
- `.upToNextMajor(from: "1.0.0")` は `from:` と同値
- `.upToNextMinor(from: "1.0.0")` は `"1.0.0"..<"1.1.0"` と同値
- 非 GitHub URL はスキップ（レジストリ非対応）
- `name:` パラメータは Swift 5.2 で追加、5.5 で非推奨
- マルチライン宣言に対応

---

## Range 上限抽出（UpdateJudge）

`VersionSpecKind::Range` の更新判定では、以下の上限構文を解釈して上限超過アップデートを除外する。

| 構文 | 例 | 上限の扱い |
|------|---|-----------|
| 比較（exclusive） | `<4.0.0` | `4.0.0` 未満 |
| 比較（inclusive） | `<=4.0.0` | `4.0.0` 以下 |
| Swift half-open | `1.0.0..<2.0.0` | `2.0.0` 未満 |
| Swift closed | `1.0.0...2.0.0` | `2.0.0` 以下 |
| Hyphen range | `1.0.0 - 2.0.0` | `2.0.0` 以下 |
| Maven range | `[1.0,2.0)`, `(,2.0]` | 末尾 `)` は未満、`]` は以下 |
| v-prefix | `<v4.0.0`, `[v1,v2)` | 比較時に `v` を除去 |


## 2026-10-22 の仕様監査

各言語の公式仕様と parser・manifest・更新判定を突き合わせた。通常形式と、解析するが安全に書き換えない形式を区別する。

| 言語・公式仕様 | 対応済みの主な構文 | 自動更新から外す形式・制限 |
|---|---|---|
| [Node](https://github.com/npm/node-semver) | exact、caret、tilde、部分指定、x-range、比較の組、hyphen、alias | protocol・dist-tag は対象外。OR は解析するが書き換えない。単独の比較付き x-range と `~>` 付き x-range は未対応（軽微な取りこぼし） |
| [Python](https://packaging.python.org/en/latest/specifications/version-specifiers/) | 比較、compatible release、prefix matching、epoch、pre/post/dev、Poetry の bare pin、extras・marker | local 固定・除外は書き換えない。Poetry の複数条件配列は対象外 |
| [Cargo](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html) | 暗黙・明示 caret、tilde、exact、wildcard、複数要件、pre/build | path・別レジストリは対象外。上限のない混合要件は安全のため書き換えない |
| [Go](https://go.dev/ref/mod) | SemVer、pseudo-version、`+incompatible`、pinned、exclude・retract の候補除外 | replace に該当する require は更新しない。範囲指定は Go 自体に存在しない |
| [Ruby](https://guides.rubygems.org/patterns/#pessimistic-version-constraint) | exact、pessimistic、比較、複合制約、ドット区切りのプレリリース | 除外制約・非レジストリのブロック・動的な DSL は自動書き換えしない |
| [Composer](https://getcomposer.org/doc/articles/versions.md) | exact、caret、tilde、wildcard、比較、hyphen、stability flag、4セグメント、patch alias | OR・除外・inline alias・開発ブランチは書き換えない。`~>` は本家が拒否する |
| [Gradle](https://docs.gradle.org/current/userguide/dependency_versions.html) | exact、prefix、Maven range、strict/prefer/reject、version catalog、変数参照 | latest・SNAPSHOT は浮動参照として保持。解決できない計算値・plugin ID は対象外 |
| [Swift](https://docs.swift.org/package-manager/PackageDescription/PackageDescription.html) | from、exact、upToNextMajor/Minor、開・閉範囲、pre/build、追加引数 | branch・revision・path・registry の id は対象外。GitHub URL の版付き依存を扱う |

### 実害を再現して修正した箇所

- 修飾子付き上限の読み落とし: `>=1.0rc1,<1.0rc3` で上限を `1.0` と読んで `1.0rc4` を選んでいた。上限の版全体を取り、6言語で解析から書き戻し・再解析まで検証する。
- Ruby の `x` 識別子: `>=1.0.a, <=1.0.x` の `.x` を npm の wildcard と誤認し、上限を `<1.1` へ広げていた。Ruby では通常の文字列セグメントとして比較し、安定版 `1.0` を選ばないようにする。
- 複数上限の順序: Composer の `<1.0.0p2 <1.0.0` を汎用の順序で選ぶと、patch alias の上限を誤る。候補選択と同じ言語別比較を使い、同値なら排他を優先する。
- 大きな release segment: `==18446744073709551616.1.*` の上限計算が `u64` への変換に失敗し、別majorを選んでいた。prefix・compatible release・部分上限の加算を10進文字列で共有し、epoch と桁上がりも検証する。

Node の上記の軽微な未対応は、安全に書き換える新規実装の対象にせず、現状の検出範囲として記録する。各言語で意図的に除外する参照を、レジストリの版へ変換する変更は行っていない。
