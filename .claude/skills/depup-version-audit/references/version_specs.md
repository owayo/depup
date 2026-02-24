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
| Dist-tag | `latest`, `next`, `canary` | タグ | Wildcard |
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
| Arbitrary equality | `===v1.2-custom` | arbitrary equality (`===`) | Exact（比較時は数値部） |
| Compatible release | `~=1.2.3` | `>=1.2.3, <1.3.0` 相当 | Tilde |
| Compatible release (2-part) | `~=1.2` | `>=1.2, <2.0` 相当 | Tilde |
| Greater or equal | `>=1.2.3` | 以上 | GreaterOrEqual |
| Greater | `>1.2.3` | 超過 | Greater |
| Less or equal | `<=1.2.3` | 以下 | LessOrEqual |
| Less | `<1.2.3` | 未満 | Less |
| Not equal | `!=1.2.3` | 除外 | Range |
| Not equal wildcard | `!=1.2.*` | メジャー.マイナー除外 | Range |
| Wildcard | `*`, `1.*` | 任意バージョン | Wildcard |
| Range (comma) | `>=1.0,<2.0` | AND 結合 | Range |
| Epoch | `>=1!2.0` | エポック接頭辞 | 演算子付きで対応（比較時除去） |
| Caret (Poetry) | `^1.2.3` | Poetry 互換（caret） | Caret |
| Tilde (Poetry) | `~1.2.3` | Poetry 互換（tilde） | Tilde |
| Environment markers | `; python_version>="3.8"` | 環境マーカー | パース後除去 |
| Extras | `package[extra]>=1.0` | extras 指定 | extras保持 |
| Local version | `==1.0+local1` | ローカル版 | Exact（比較時は数値部） |
| Pre-release | `==1.0a1`, `==1.0b1`, `==1.0rc1` | PEP 440 プレリリース | Exact（比較時は数値部） |
| Post-release | `==1.0.post1` | ポストリリース | Exact（比較時は数値部） |
| Dev release | `==1.0.dev1` | 開発リリース | Exact（比較時は数値部） |

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
| Build metadata | `1.0.0+build` | ビルドメタデータ | 未対応（現実装） |

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
- `exclude` ディレクティブはスキップ
- `retract` ディレクティブはスキップ（`retract [v1.0, v1.9]` のレンジ形式あり）

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
| Stability flag | `1.0@dev` | 安定性フラグ | @以降除去 |
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
| Strict version | `!!1.2.3` | 厳密バージョン | ー |
| Reject version | `1.2.+` reject `1.2.5` | 拒否指定 | ー |
| Prefer version | `1.+` prefer `1.2.3` | 優先指定 | ー |
| Version catalog | `libs.xxx` | バージョンカタログ | ー |
| Platform | `platform()`, `enforcedPlatform()` | BOM/Platform | ー |
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

**公式仕様**: [Swift Package Manager](https://developer.apple.com/documentation/packagedescription/package/dependency)

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
