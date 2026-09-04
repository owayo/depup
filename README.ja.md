<p align="center">
  <img src="docs/images/app.png" width="128" alt="depup">
</p>

<h1 align="center">depup</h1>

<p align="center">
  マルチ言語依存関係アップデートCLIツール
</p>

<p align="center">
  <a href="https://github.com/owayo/depup/actions/workflows/release.yml"><img src="https://github.com/owayo/depup/actions/workflows/release.yml/badge.svg?branch=main" alt="Release"></a>
  <a href="https://github.com/owayo/depup/actions/workflows/ci.yml"><img src="https://github.com/owayo/depup/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/owayo/depup/releases"><img src="https://img.shields.io/github/v/release/owayo/depup" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

<h3 align="center">対応言語</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Node.js-339933?logo=nodedotjs&logoColor=white" alt="Node.js">
  <img src="https://img.shields.io/badge/Python-3776AB?logo=python&logoColor=white" alt="Python">
  <img src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/Go-00ADD8?logo=go&logoColor=white" alt="Go">
  <img src="https://img.shields.io/badge/Ruby-CC342D?logo=ruby&logoColor=white" alt="Ruby">
  <img src="https://img.shields.io/badge/PHP-777BB4?logo=php&logoColor=white" alt="PHP">
  <img src="https://img.shields.io/badge/Java-ED8B00?logo=openjdk&logoColor=white" alt="Java">
  <img src="https://img.shields.io/badge/Swift-F05138?logo=swift&logoColor=white" alt="Swift">
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>

---

### 出力例

<table>
  <tr>
    <td align="center">
      <strong>Python (pyproject.toml)</strong><br>
      <img src="docs/images/output_python.png" width="400" alt="depup Python出力">
    </td>
    <td align="center">
      <strong>Tauri (package.json + Cargo.toml)</strong><br>
      <img src="docs/images/output_tauri.png" width="400" alt="depup Tauri出力">
    </td>
  </tr>
</table>

## 特徴

- **マルチ言語対応**: Node.js, Python, Rust, Go, Ruby, PHP, Java, Swift
- **mise 対応**: `mise.toml` / `.tool-versions` のツールバージョンも同じワークフローで更新
- **マニフェスト更新**: マニフェストファイル内のバージョン指定を直接更新
- **スマートバージョン処理**: バージョン範囲形式（^, ~, >=）を維持しつつ、上限を壊さずに更新
- **固定バージョン検出**: 意図的に固定されたバージョンはデフォルトでスキップ
- **エイジフィルター**: N日/週前以降にリリースされたバージョンのみに更新
- **pnpm連携**: pnpm設定の `minimumReleaseAge` を自動適用
- **Bun Catalogs対応**: `package.json` の Bun `catalog` / `catalogs` 定義を更新
- **モノレポ対応**: `.depup`、Cargo/pnpm/Go ワークスペース、Gradle マルチプロジェクト、ネストした package install、Tauriプロジェクト
- **リリース日表示**: 各バージョンのリリース日時を表示
- **複数出力形式**: テキスト（カラー）、JSON、diff

## 対応言語

| 言語 | マニフェスト | レジストリ | ロックファイル |
|------|-------------|----------|---------------|
| <img src="https://img.shields.io/badge/-339933?logo=nodedotjs&logoColor=white" height="16"> Node.js | package.json（Bun catalogs 含む） | npm | package-lock.json, pnpm-lock.yaml, yarn.lock, bun.lock, bun.lockb |
| <img src="https://img.shields.io/badge/-3776AB?logo=python&logoColor=white" height="16"> Python | pyproject.toml | PyPI | uv.lock, requirements.lock, poetry.lock |
| <img src="https://img.shields.io/badge/-000000?logo=rust&logoColor=white" height="16"> Rust | Cargo.toml | crates.io | Cargo.lock |
| <img src="https://img.shields.io/badge/-00ADD8?logo=go&logoColor=white" height="16"> Go | go.mod（go.work のメンバーも自動検出） | Go Proxy | go.sum |
| <img src="https://img.shields.io/badge/-CC342D?logo=ruby&logoColor=white" height="16"> Ruby | Gemfile | RubyGems | Gemfile.lock |
| <img src="https://img.shields.io/badge/-777BB4?logo=php&logoColor=white" height="16"> PHP | composer.json | Packagist | composer.lock |
| <img src="https://img.shields.io/badge/-ED8B00?logo=openjdk&logoColor=white" height="16"> Java | build.gradle, build.gradle.kts, gradle/*.versions.toml（settings.gradle のサブプロジェクトも自動検出） | Maven Central | gradle.lockfile |
| <img src="https://img.shields.io/badge/-F05138?logo=swift&logoColor=white" height="16"> Swift | Package.swift | GitHub Tags | Package.resolved |
| mise | mise.toml, .mise.toml, .config/mise/config.toml, .tool-versions | `mise ls-remote` | mise.lock |

## 動作要件

- **OS**: macOS, Linux, Windows
- **Rust**: 1.85以上（ソースからビルドする場合）
- **mise**: mise のツールバージョンを更新する場合のみ必要（バージョン一覧は `mise ls-remote` から取得します）。未インストールの環境では mise の設定ファイルをスキップし、警告を 1 回表示して他の言語の更新を続行します

## インストール

### Homebrew (macOS/Linux)

```bash
brew install owayo/depup/depup
```

### ソースから

```bash
git clone https://github.com/owayo/depup.git
cd depup
cargo install --path .
```

### GitHubリリースから

[Releases](https://github.com/owayo/depup/releases)から最新のバイナリをダウンロードできます。

#### macOS (Apple Silicon)

```bash
curl -L https://github.com/owayo/depup/releases/latest/download/depup-aarch64-apple-darwin.tar.gz | tar xz
sudo mv depup /usr/local/bin/
```

#### macOS (Intel)

```bash
curl -L https://github.com/owayo/depup/releases/latest/download/depup-x86_64-apple-darwin.tar.gz | tar xz
sudo mv depup /usr/local/bin/
```

#### Linux (x86_64)

```bash
curl -L https://github.com/owayo/depup/releases/latest/download/depup-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv depup /usr/local/bin/
```

#### Linux (ARM64)

```bash
curl -L https://github.com/owayo/depup/releases/latest/download/depup-aarch64-unknown-linux-gnu.tar.gz | tar xz
sudo mv depup /usr/local/bin/
```

#### Windows

`depup-x86_64-pc-windows-msvc.zip` を [Releases](https://github.com/owayo/depup/releases) からダウンロードし、展開してPATHに追加してください。

## クイックスタート

```bash
# 全ての依存関係を更新（ドライラン）
depup -n

# Node.jsの依存関係のみ更新
depup --node

# エイジフィルター付きで更新（2週間以上）
depup --age 2w

# diffを表示して更新
depup --diff
```

## 使い方

### 基本構文

```bash
depup [OPTIONS] [PATH]
```

### オプション

| オプション | 短縮形 | 説明 |
|-----------|-------|------|
| `--cd <DIR>` | `-C` | 指定ディレクトリに移動してから実行 |
| `--dry-run` | `-n` | 変更せずに更新内容を表示 |
| `--verbose` | | 詳細出力を有効化 |
| `--quiet` | `-q` | 最小限の出力 |
| `--node` | | Node.jsの依存関係のみ更新 |
| `--python` | | Pythonの依存関係のみ更新 |
| `--rust` | | Rustの依存関係のみ更新 |
| `--go` | | Goの依存関係のみ更新 |
| `--ruby` | | Rubyの依存関係のみ更新 |
| `--php` | | PHPの依存関係のみ更新 |
| `--java` | | Javaの依存関係のみ更新 |
| `--swift` | | Swiftの依存関係のみ更新 |
| `--mise` | | mise（mise.toml / .tool-versions）のツールバージョンのみ更新 |
| `--exclude <PKG>` | | 特定パッケージを除外（複数指定可） |
| `--only <PKG>` | | 特定パッケージのみ更新（複数指定可） |
| `--include-pinned` | | 固定バージョンも更新対象に含める |
| `--age <DURATION>` | | 最小リリース経過期間（例: 2w, 10d, 1m）。グローバル設定を上書き |
| `--no-age` | | このランのみ age フィルターを無効化（グローバル設定とデフォルトを上書き） |
| `--osv` | | OSV.dev の脆弱性データベースで candidate を照会し、既知の脆弱性があるバージョンをスキップ（デフォルトで有効） |
| `--no-osv` | | このランのみ OSV 脆弱性チェックを無効化（グローバル設定とデフォルトを上書き） |
| `--max-change <LEVEL>` | | 許容するアップデートの上限: `patch`（patch のみ）/ `minor`（patch + minor）/ `major`（デフォルト＝全許可） |
| `--json` | | JSON形式で出力 |
| `--diff` | | diff形式で変更を表示 |
| `--install` | | 更新後にパッケージマネージャのinstallを実行 |
| `--version` | `-V` | バージョンを表示 |
| `--help` | `-h` | ヘルプを表示 |

### 使用例

```bash
# 全ての更新をプレビュー
depup -n

# lodashとtypescriptのみ更新
depup --only lodash --only typescript

# reactを更新から除外
depup --exclude react

# 同じパッケージが exclude にあっても only が優先される
depup --only lodash --exclude lodash

# 2週間以上経過したパッケージのみ更新
depup --age 2w

# PythonとRustのみ更新
depup --python --rust

# Java（Gradle）の依存関係のみ更新
depup --java

# Swift（Package.swift）の依存関係のみ更新
depup --swift

# mise（mise.toml / .tool-versions）のツールバージョンのみ更新
depup --mise

# CI/CD用にJSON出力
depup --json

# 更新後にnpm installを実行
depup --node --install

# 別のディレクトリで実行
depup --cd ./projects/myapp -n
```

## バージョン処理

### 固定バージョン（デフォルトで除外）

固定バージョンは意図的に固定されているため、デフォルトで更新から除外されます：

| 言語 | 固定の例 | 更新 |
|------|---------|------|
| Node.js | `"1.2.3"` | ❌ |
| Node.js | `"^1.2.3"`, `"~1.2.3"` | ✅ |
| Python | `"==1.2.3"` | ❌ |
| Python | `">=1.2.3"`, `"^1.2.3"` | ✅ |
| Rust | `"=1.2.3"` | ❌ |
| Rust | `"1.2.3"`, `"^1.2.3"` | ✅ |
| Go | `// pinned` コメント | ❌ |
| Ruby | `'= 1.2.3'` | ❌ |
| Ruby | `'~> 1.2.3'`, `'>= 1.2.3'` | ✅ |
| PHP | `"1.2.3"` | ❌ |
| PHP | `"^1.2.3"`, `"~1.2.3"` | ✅ |
| Java | Gradleの固定バージョン | ❌ |
| Java | Gradleの strict 記法（`1.2.3!!`） | ❌ |
| Java | Maven Hard requirement（`[1.0]`） | ❌ |
| Swift | `exact: "1.2.3"` | ❌ |
| Swift | `from: "1.2.3"`, `.upToNextMinor` | ✅ |

`--include-pinned` で固定バージョンも更新対象にできます。

> **注意**: Goの依存関係は `--include-pinned` フラグに関係なく常に更新対象に含まれます。これは `go.mod` が正確なバージョンのみをサポートし、`^` や `~` のような範囲指定子がないためです。Goのすべてのバージョンは本質的に「固定」されています。
>
> **注意**: Go の `exclude` ディレクティブは記述自体を書き換えず、更新候補の除外に反映します。上流モジュールの最新 `go.mod` が単一版または閉区間で `retract` した版も候補から除外します。タグ付きバージョンがないモジュールでは Go Proxy の `@latest` へフォールバックし、`.info` の `Time` が省略されている場合は Unix epoch を使うため、公開日不明の版が `--age` で永久に除外されることはありません。
>
> **注意**: `gem "pg", ">= 0.18", "< 2.0"` のような Gemfile の複合制約は解析・更新の対象です。包含下限だけを進め、元の引数の個数・順序・クォート種別・空白・括弧付き呼び出し形式・行末条件修飾子を保ったまま書き戻します。比較の基準は記述順に依存せず包含下限なので、`gem "pg", "< 2.0", ">= 0.18"` でも `0.18` を基準に判定します。書き換え後の制約を元の引数の個数へ分割できない場合（引数自体がカンマを含む場合など）は、unsafe な編集を適用せずエラーとして報告します。`gem "rack", "!= 2.2.4"` のような除外制約は、一部だけを更新すると意味が壊れるため判定の段階でスキップします。
>
> **注意**: `git_source(:name) { ... }` で登録した Bundler のカスタム git source ショートハンド（例: `gem 'rails', stash: 'forks/rails'`）も、組み込みの `git:` / `github:` と同じくレジストリ外依存として更新対象から除外します。
>
> **注意**: バージョンなしで `git:` / `github:` / `bitbucket:` / `gist:` / `path:` / `source:` を指定した Gemfile 依存は、RubyGems のレジストリ制約へ変換せずにスキップします。同じ形式でもバージョンが明示されていれば、Bundler が gemspec を検証する制約として、source オプションを保持したまま解析・更新できます。オプションキーは Ruby の 2 通りの綴り（`git: '...'` と hash-rocket 形式 `:git => '...'`）の両方を認識します。`git ... do` / `github ... do` / `path ... do` / `source ... do` ブロック内の gem も同じ理由でスキップし、`platforms` / `install_if` のような通常のブロックは従来どおり処理します。引数が次行へ続く宣言（`gem "devise",`）は、その行だけでは版を決められないため「バージョンなしのレジストリ依存」として報告せずスキップします。行単位の `group:` / `groups:` オプションは開発依存の判定に使います。
>
> **注意**: Gemfile の依存宣言は、一般的な Ruby DSL 形式（`gem "rack", "~> 3.0"`）と括弧付きメソッド呼び出し形式（`gem("rack", "~> 3.0")`）のどちらも解析・更新できます。更新時は元の呼び出し形式を保持します。同じ gem が複数箇所（例: トップレベルと `group :test` ブロックの両方）に宣言されている場合は、曖昧な書き込みとして拒否します。
>
> **注意**: `alias = { package = "actual-crate", version = "1" }` のような Cargo のリネーム依存は、実パッケージ名で取得し、マニフェスト上のキーへ書き戻します。`--only` / `--exclude` はどちらの名前でも指定できます。
>
> **注意**: `--only` が指定されている場合は `--exclude` より優先されます。明示的に許可したパッケージは、同じ名前が広い除外リストに含まれていても更新対象に残ります。
>
> **注意**: Composer の platform package（`php`, `hhvm`, `ext-*`, `lib-*`, Composer API パッケージなど）は更新対象から除外します。
>
> **注意**: Composer/Packagist は `composer/semver` の `VersionParser` に従って 1〜4 セグメントの数値バージョンを valid 扱いします。depup も `1.2.3.4`、`^1.0.0.0`、`~3.4.5.6`、`1.0.0.*` などの 4 セグメントまでのバージョンをパース・更新でき、5 セグメント以上は invalid として除外します。
>
> **注意**: Composer の modifier は区切り文字を省略でき `.` / `_` も使えるため（`composer/semver` は `[._-]?`）、depup は `5.0.0alpha3` / `1.0.0.RC1` / `1.0.0_beta1` をプレリリース、`2.2.1p1` / `2.2.1pl1` / `2.2.1patch1` を base より **新しい** patch alias として扱います。どちらの形式も現在の Packagist に実在します（`nikic/php-parser` の `5.0.0beta1`、`laminas/laminas-diactoros` のセキュリティパッチ `2.2.1p2`）。
>
> **注意**: Gradle の `-SNAPSHOT` / `.SNAPSHOT` は更新対象から除外します。SNAPSHOT は解決のたびに最新のタイムスタンプ版を取り直す「移動する参照」なので、固定 release へ書き換えるとビルドが使うものが黙って変わってしまいます。`.Final` / `.RELEASE` / `-jre` / `-SP1` のような安定版 qualifier は従来どおり更新します。
>
> **注意**: Gradle の `resolutionStrategy { force ... }` / `constraints { }` / `dependencySubstitution { }` の中に書かれた座標は依存として報告しません。これらは他所で宣言済みのバージョンを再掲するものであり、別々の宣言として扱うと同じ座標が曖昧になって更新が拒否されていました。
>
> **注意**: Go の `+incompatible` は `go` コマンドと同じ規則でフィルタします。semver 順で最初の `+incompatible` に到達したとき、直前の compatible 版が本物の `go.mod` を持っていれば、それ以降の `+incompatible` をすべて捨てます。これが無いと `github.com/libp2p/go-libp2p` が `v0.49.0` から 2018 年の `v6.0.23+incompatible` へ「更新」され、しかもビルドは通ってしまいます。
>
> **注意**: PyPI 以外を既定インデックスにしている `pyproject.toml`（Poetry の `priority = "primary"` / `"default"` source、uv の `[[tool.uv.index]] default = true` や `[tool.uv] index-url`、PDM による `pypi` の上書き）は、警告を出したうえで全依存をスキップします。depup は PyPI しか参照しないため、更新すると private パッケージが同名の公開パッケージで置き換わってしまいます。

### 範囲形式の維持

depupは元のバージョン範囲形式を維持します：

```
"^1.2.3" → "^2.0.0"  （キャレット維持）
"~1.2.3" → "~1.3.0"  （チルダ維持）
"~1.2"   → "~1.9"    （チルダのセグメント数維持 — 増やすと許容幅が狭まる）
"~1"     → "~2"      （1 セグメントのチルダはメジャー幅を維持）
"~1.2 <2.0.0" → "~1.9 <2.0.0" （comparator set に埋め込まれたチルダもセグメント数を維持）
"~1, <5.0" → "~4, <5.0" （Cargo の複数要件でもチルダの幅を維持）
"~> 7.0" → "~> 8.1"  （RubyGems の悲観的演算子、セグメント数維持）
">=1.0.0" → ">=2.0.0" （範囲維持）
"requests (>=2.28,<3); python_version < '3.12'" → "requests (>=2.31,<3); python_version < '3.12'" （PEP 508 の括弧とマーカーを維持）
"coverage [toml] >=7,<8" → "coverage [toml] >=7.6,<8" （PEP 508 extras の空白を維持）
"'paramiko>=3.5.0,<4.0.0,'" → "'paramiko>=3.9.1,<4.0.0,'" （PEP 508 の末尾カンマを維持）
"'paramiko>=3.5.0,<4.0.0'" → "'paramiko>=3.9.1,<4.0.0'" （TOML リテラル文字列の引用符を維持）
"1.x" → "2.x" （ワイルドカード形式を維持）
"1.2.x - 2.3.x" → "1.9.x - 2.3.x" （npm のハイフンレンジ、x-range 端点）
"1.x.x" → "2.x.x" （複数のワイルドカード位置を維持）
"1.2.*" → "1.3.*" （ワイルドカード形式を維持）
"v1.*" → "v2.*" （先頭の `v` を維持）
"V1.*" → "V2.*" （Composer の大文字 `V` を維持）
"^1.x" → "^2.x" （npm の caret + x-range、演算子を維持）
"~1.2.x" → "~2.3.x" （npm の tilde + x-range、演算子を維持）
"=1.x" → "=2.x" （npm の equality + x-range、演算子を維持）
"=1.2" → "=2.3" （npm の partial comparator、演算子を維持）
"5.3.+" → "5.4.+" （Gradle プレフィックスを維持）
"5.3.+!!" → "6.1.+!!" （Gradle strict 動的プレフィックスを維持）
"1.2.3!!" → "2.0.0!!" （Gradle strict を維持）
"[1.7, 1.8[!!" → "[1.7.36, 1.8[!!" （prefer なしの Gradle strict 範囲）
"[1.0]" → "[2.0]" （Maven Hard requirement を維持）
"[1.2.3.Final]" → "[1.3.0]" （qualifier 付き Maven Hard requirement）
group = "com.google.guava", name = "guava", version = "32.1.2-jre" → version = "33.4.0-jre" （Gradle Kotlin map 記法）
junit = "junit:junit:4.13.2" → "junit:junit:4.13.3" （Gradle version catalog の library）
guava = "32.1.2-jre" → "33.4.0-jre" （Gradle version catalog の version 参照）
prefer("1.7.25") → prefer("1.7.36") （Gradle rich version の strict 範囲内の prefer）
"org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.25" → "org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.36" （Gradle strict range の prefer 短縮記法）
"group:name:1.0.0:classifier@zip" → "group:name:1.1.0:classifier@zip" （Gradle classifier/extension を維持）
```

`"*"`、npm の dist-tag（`"latest"` など）、Gradle の動的指定（`"latest.release"`、`"latest.integration"`、`"latest.milestone"`、ユーザ定義 `latest.<status>` など全般）のような完全浮動指定は、厳密バージョンへ変質させないため更新対象から除外されます。Composer の `*.*` / `v*` / `V*` / `x.x` のように数値アンカーを持たない多セグメントワイルドカード、および Java/Gradle の `[,]` / `(,)` のように下限・上限とも空の Maven レンジも同様に弾かれます（phantom update や「常に古い」と誤判定する原因になるため）。`1.x.3` や `^x.0.0` のようにワイルドカード文字 (`x`/`X`/`*`) の後ろに数値セグメントが続く形式は node-semver / semver で invalid な x-range なのでパース時点で除外します。

npm の semver トークンは、更新対象として解析する前に prerelease / build metadata 識別子を検証します。アンダースコアを含む識別子（`1.2.3-rc_1`）、空識別子を含む形式（`1.2.3-alpha..1`）、先頭ゼロを持つ数値 prerelease 識別子（`1.2.3-01`）は、不正な package.json 制約へ正規化せずスキップします。

先頭ゼロの検証は build metadata を切り落としてから行います。SemVer では build 識別子にハイフンや先頭ゼロを含められるため、`1.0.0+2024-01` や `1.2.3+00` のようなバージョンは valid として扱い、更新対象に残します（検証されるのは prerelease 部のみです）。

npm の partial comparator では、`=1.2` や `=1` を固定バージョンではなく node-semver の部分バージョン規則として扱います。depup は `=` 演算子と見えているセグメント数を保って更新します（`=1.2` → `=2.3`、`=1` → `=2`）。

更新候補の順序はエコシステム別に比較します。Node.js / Rust / Go / Swift は数値プレリリース（`1.0.0-1`）を含む SemVer を使い、優先順位の比較ではビルドメタデータを無視するため、`1.1.3` と `1.1.3+spec-1.1.0` の違いだけでは更新しません。Python は PEP 440 正規化、Ruby は RubyGems のセグメント順（英字またはハイフンを含む版はプレリリース）、Composer は対応する安定版より新しい patch alias（`-p1` / `-pl1` / `-patch1`）、Java は Gradle 公式のバージョン順序を使います。数値セグメントは固定整数サイズの上限なしで比較します。

Node.js は node-semver 互換の従来形式 `~>1.2.3` も受理し、更新後も `~>` を保持します。Composer は明示的な等価演算子（`=1.2.3` / `==1.2.3`）を保持して更新します。除外指定の `<>1.2.3` は解析しますが、安全のため自動書き換えません。

Gradle の `strictly` / `require` / `prefer` / `reject` を使う rich version 宣言は、`implementation("org.slf4j:slf4j-api") { version { ... } }` のような依存ブロック内でも解析対象になります。文字列記法の短縮構文は、`group:name:1.2.3!!`、`group:name:5.3.+!!`、`group:name:[1.7, 1.8[!!`、prefer 付き strict 範囲の `group:name:[1.7, 1.8[!!1.7.25` のように、固定値・動的プレフィックス・範囲を解析できます。`strictly` または `require` が範囲を指定し、`prefer` が選好バージョンを指定している場合、depup は範囲を上限制約として維持しつつ `prefer` の値を更新します。`reject` に列挙されたバージョンは更新候補から除外され、`2.+` のような動的 reject と `[1.5,1.9)` のようなレンジ reject も考慮します。

Gradle の宣言ラッパ `platform(...)` / `enforcedPlatform(...)` / `testFixtures(...)` に対応しています。`implementation platform('com.google.cloud:libraries-bom:26.1.0')` や `testImplementation(platform("org.junit:junit-bom:5.10.0"))` のような BOM 宣言も解析・更新でき、dev / 本番の判定にはラッパではなく本来の configuration 名を使います。`ext.<name> = '...'` / `project.ext.<name> = "..."` のドット代入も `ext { ... }` ブロックと同様に変数として解決し、`${Versions.retrofit}` のような修飾付き参照は最終セグメントで解決します。同じ短名が異なる値で複数定義されている場合は、別オブジェクトの値を拾う誤更新を避けて更新しません。

`gradle/*.versions.toml` にある Gradle version catalog は Java マニフェストとして検出されます。depup は `[libraries]` の `alias = "group:name:version"`、`module = "group:name"`、`group` / `name` / `version`、`version.ref` を解析し、参照先の `[versions]` もその場で更新します。`strictly` / `require` / `prefer` / `reject` / `rejectAll` を含む rich version table は Gradle build ファイルと同じ候補選別ルールで扱います。`[plugins]` は Gradle plugin ID で Maven Central 座標と一致しないため更新対象から除外します。

Python の compatible release 句は PEP 440 に従います。`~=1.2`（= `>=1.2,<2.0`）と `~=1.2.3`（= `>=1.2.3,<1.3.0`）は明示的な上限を持つレンジとして扱われ、互換範囲内に留まって更新されます（`~=1.2.3` は 1.2 系、`~=1.2` は 1.x 系に留まり major/minor を跨ぎません）。書き換えは元のセグメント数を維持します（`~=1.2` → `~=1.9`。`~=1.9.0` にすると上限が `<1.10.0` に変わってしまうため）。単一セグメントの無効な形式である `~=1` はスキップします。

PEP 440 の prefix matching は、`==1.2.*` / `!=1.2.*` のような release segment の `==` / `!=` 指定だけで受け付けます。`>=1.0.*`、`~=1.0.*`、`==1.0a1.*`、`==1.0.post1.*`、`==1.0+local.*` のような無効形はパース時点でスキップします。任意一致の `===1.0.*` は prefix matching ではなく固定指定として扱います。

JVM 系の milestone 版はプレリリースとして扱われ、安定版の更新候補から除外されます: `4.0.0-M1`、旧 Spring Boot のドット区切り `2.0.0.M1`、綴りきりの `-milestone1`。これがないと `assertj-core 3.24.2` が `4.0.0-M1` へ、`junit-bom 5.10.0` が `5.13.0-M3` へ、`spring-core 5.3.23` が `7.0.0-M6` へ更新されてしまいます。判定は「`m` の直後が数字」のトークンに限定するため、JVM の安定版 qualifier（`.Final`、`-jre`、`-android`、`.RELEASE`、`.GA`、`-SP1`）や `-macos1` のような識別子を誤判定することはありません。他のプレリリースと同様、現在版が milestone の場合は候補に milestone を残すため、次の milestone へ進めます。

PEP 440 のプレリリースは、セパレータなしで書かれた場合（例: `2.0.0rc1`, `1.0rc1`, `1.0.0a1`）でも検出され、デフォルトで除外されます。これにより安定版の依存がリリース候補へ誤って更新されることはありません。ポストリリース（`1.0.post1`）は対応する安定版より新しいと比較され、エポック（`1!2.3`）は比較時に最優先されます。プレリリースに付随する post リリース（`1.0a1.post1`）も元のプレリリースより新しいと扱われ（`1.0a1 < 1.0a1.post1 < 1.0`）、α 版を追跡しているユーザーが post の修正を取り逃がすことはありません。

PEP 440 の local version は、semver の build metadata ではなく Python 固有のバージョン意味論として扱います。固定指定と除外指定では local label を保持します（`==1.0+cu121`, `!=1.0+local1`）。Python の候補選択では local version を同じ public version より新しいものとして扱い（`1.0+local > 1.0`）、local segment も PEP 440 に従って比較します（`1.0+1 > 1.0+abc`, `1.0+abc.2 > 1.0+abc.1`）。PEP 440 で local label が許可されない ordered / compatible 指定（`>=1.0+local`, `~=1.0+local`, `>=1.0+local,<2.0`）はスキップします。

Poetry の `[tool.poetry.dependencies]` では、演算子なしのバージョン文字列（`requests = "2.28.0"`）は完全一致ピンです（Poetry 公式の "Exact requirements"、`==2.28.0` と同義）。depup は simple 形式・inline table（`{ version = "1.26.0", optional = true }`）の両方でこれを exact/pinned な依存として解析するため、明示 `==2.28.0` と同じく依存として現れ（pinned として表示され、`--include-pinned` で更新可能）、更新時は演算子を付けずに新バージョンへ書き換えます（`4.2.1` → `5.0.0`）。この bare-exact 解釈は Poetry コンテキストに限定され、pip / PEP 508 の依存指定は演算子必須のため、そちらでは bare バージョンを受理しません。

### 範囲制約

depupは上限付きの範囲制約（排他的・包含的の両方）を尊重します：

```
">=3.5.0,<4.0.0"   → ">=3.9.1,<4.0.0"
">=1.0,<=2.0"      → ">=2.0,<=2.0"
"4.0.0..<5.0.0"    → "4.99.0..<5.0.0"
"4.0.0...4.9.9"    → "4.9.9...4.9.9"
"1.2.0 - 2.0.0"    → "1.9.3 - 2.0.0" （npmハイフン範囲）
"1.0 - 2.0"        → "2.0.9 - 2.0" （npm/Composer の部分上限は `<2.1` に展開）
"[1.0,2.0)"        → "[1.9.3,2.0)" （Mavenスタイル）
"[1.0,2.0]"        → "[2.0,2.0]" （Mavenスタイル）
"[1.0,2.0.Final)"  → "[1.9.3,2.0.Final)" （Maven qualifier）
"[1.0,2.0-beta1-SNAPSHOT)" → "[1.9.3,2.0-beta1-SNAPSHOT)" （複数区切りの Maven qualifier）
"[1.0,2.0["        → "[1.9.3,2.0[" （Maven上限排他ブラケット）
"<4.0.0"           → スキップ（上限のみの制約）
">1.0.0"           → スキップ（排他的下限）
"]1.0,2.0["        → スキップ（Maven の排他的下限）
```

npm/Composer のハイフンレンジでは、右辺が `1.0 - 2.0` のような部分指定のときにワイルドカード展開後の排他的上限として解釈します。そのため `2.0.x` は更新候補に含まれ、`2.1.0` 以降は除外されます。

node-semver の `HYPHENRANGE` は両端に `XRANGEPLAIN` を許すため、`1.x - 2.x` のような x-range 端点も valid として解析・更新します。depup が他の経路で弾いている端点（`1.x.3` のようにワイルドカードの後ろに数値が続く形式、数値アンカーのない `*`）は引き続き弾きます。

Composer は `~>` 演算子を拒否する（`Invalid operator "~>"`）ため、PHP では `~>` をスキップし、Composer が読めない制約を書き戻さないようにしています。node-semver は `~>` を valid とするので Node では従来どおり受理します。

Cargo の `registry-index = "..."` 付き依存は、`registry = "..."` と同じく crates.io 以外を指すため更新対象から除外します。


依存関係に上限付きレンジ（例：`>=3.5.0,<4.0.0`、`>=1.0,<=2.0`、`4.0.0...4.9.9`）がある場合、depup は：
- 上限を超えるバージョンを**提案しません**
- 包含的上限（`<=`、`...`）は上限値自体を候補に含めます
- マニフェストファイル内の元の制約形状を**維持**します
- 指定範囲内で互換な最新バージョンに合わせて、**下限側だけを更新**します

安全に書き換えられない制約は、部分更新せずにスキップします。例として npm/Composer の OR 制約（`^1 || ^2`）と Composer の後方互換表記である単一パイプ（`^1 | ^2`）、`!=` を含む除外制約（`!=1.2.3`、`>=1.0, !=1.5.0, <2.0`）、上限のみの制約（`<4.0.0`、`<=2.0`）、厳密な下限制約（`>1.0.0`）、下限を持たない Maven 形式レンジ（`(,2.0]`）、Maven の排他的下限レンジ（`]1.0,2.0[`）などが該当します。Composer（composer/semver）は not-equal を `<>` でも綴れるため、`>=1.0 <>1.5.0 <2.0` / `>=1.0,<>1.5.0,<2.0` のような `<>` 除外制約も `!=` 版と同様にスキップします。

JSON マニフェストでは、depup が解析対象にする依存セクションだけを書き換えます。`package.json` の `overrides`、`composer.json` の `replace` / `provide` / `conflict` などは変更しません。

同じ依存キーがマニフェスト内に複数回宣言されている場合や、複数の Gradle 依存が一つのバージョン変数または version catalog の `version.ref` を共有している場合、depup は曖昧な書き込みを拒否します。これにより、スキップまたは固定された別の宣言を暗黙に変更しません。

Bun ワークスペースでは、root `package.json` のトップレベル `catalog` / `catalogs` と `workspaces.catalog` / `workspaces.catalogs` を解析・更新します。ワークスペース package の `"react": "catalog:"` や `"jest": "catalog:testing"` のような参照は `catalog:` のまま保持し、共有 catalog 定義側のバージョンだけを更新します。`pnpm-workspace.yaml` に定義される pnpm catalogs はまだマニフェストとして解析しないため、pnpm catalogs に裏付けられた package.json の `catalog:` 参照は安全側でスキップします。

TOML マニフェストでは、基本文字列（`"..."`）とリテラル文字列（`'...'`）のどちらも、対応する依存セクション内では引用符を維持して更新します。`Cargo.toml` では `[dependencies]`、`[dev-dependencies]`、`[build-dependencies]`、`[workspace.dependencies]`、target 固有の依存テーブルだけを書き換え、metadata テーブルは変更しません。Cargo git tag 更新も inline table と複数行テーブルの両方で同じセクション制限に従い、単一引用符・二重引用符を保持します。`[patch.<registry>]` / `[patch.<registry>.<package>]` も更新対象です。`crates-io` 以外の `registry` を指定した Cargo 依存は、depup が crates.io だけを問い合わせるためスキップします。Cargo の比較レンジは `>=1.0, <2.0, >=1.0.100` のように3個以上のカンマ区切り requirement にも対応します。`^1.2.2, <1.5` のように caret/tilde/wildcard と comparator を混在させた複数要件も `semver::VersionReq` で valid 性を確認して Range として検出します（以前は黙ってスキップしていました）。上限のない複数下限の混在で安全に書き換えられないものは Skip として可視化します。

PEP 621 / PEP 735 / Poetry に加えて、uv 旧形式の `[tool.uv] dev-dependencies` と PDM の `[tool.pdm.dev-dependencies]` も読み取ります。`[tool.uv.sources]` で PyPI 以外を指す依存（`workspace = true` / `git` / `path` / `url` / `pypi` 以外の `index`）は、workspace メンバーやカスタムインデックスを PyPI の同名パッケージで上書きしないようスキップします。Poetry の多行依存テーブル（`[tool.poetry.dependencies.<name>]` / `[tool.poetry.group.<g>.dependencies.<name>]`）と TOML のクォート付きキー（`"zope.interface"` / `"ruamel.yaml"`）も解析・更新します。ドットを含む名前は TOML でクォートが必須なため、パーサが読む範囲と書き換え範囲を一致させています。

`[project]` / `[tool.rye]` / `[tool.uv]` セクションでは `dependencies` / `dev-dependencies` 配列だけを書き換え、`name` / `description` / `keywords` 等のメタデータ文字列は PEP 508 依存指定に見えても書き換えません。Python の PEP 508 version list は `>=3.5,<4,` のような末尾カンマを許容します。depup は下限更新時にもこのカンマを維持します。`pypi` 以外の `source` を指定した Poetry 依存は、PEP 621 依存を `tool.poetry.dependencies` で補足している場合も含め、depup が PyPI だけを問い合わせるためスキップします。Poetry のマルチプル制約配列形式（`foo = [{version = "<=1.9", python = ">=3.6,<3.8"}, {version = "^2.0", python = ">=3.8"}]`）も、要素ごとの `requires_python` 解決を伴わずに配列要素を安全に書き換えられないためスキップします。

Gradle の文字列記法では `:resources@zip` や `@aar` のような classifier / extension サフィックスを維持します。`//` 行コメントや `/* ... */` ブロックコメント内だけにある依存宣言は更新対象にしません。Gradle version catalog では、バージョンが宣言されている TOML の文字列形式または table 形式を維持して更新します。

npm の comparator set では、`1.2 <2.0.0` のような bare partial lower bound も扱い、下限側を更新するときは partial の形を維持します。

Swift の GitHub 依存では、HTTPS URL、scp形式のSSH URL（`git@github.com:owner/repo.git`）、標準SSH URL（`ssh://git@github.com/owner/repo.git`）、GitHubのSSH over 443 URL（`ssh://git@ssh.github.com:443/owner/repo.git`）を解析できます。GitHub タグとして `v1.2.3` と `V1.2.3` の両方を認識する一方、`Package.swift` の version requirement 文字列は厳格な SemVer (`X.Y.Z`、先頭ゼロなし) として検証します。また、`Package.swift` では `//` 行コメントや `/* ... */` ブロックコメント内に書かれた依存宣言は解析対象から除外します。
SPM の semver 2.0.0 仕様に合わせ、プレリリース識別子付きバージョン（`1.0.0-beta.1`）、ビルドメタデータ付き（`1.0.0+build.123`）、両者を組み合わせた形式（`1.0.0-rc.1+sha.abc`）も解析・更新できます。
`.package(...)` の末尾に `traits: [...]`（SPM 6.1 の Package Traits）や `moduleAliases: [...]` のような追加引数があっても、version requirement だけを置換して追加引数を保持します。Swift Package Registry の `id:` 依存（`.package(id: "scope.name", ...)`）は registry API アダプタが未実装のため現状未対応でスキップされます（GitHub URL 依存のみ対象。将来対応予定）。

`go.mod` では、`) // direct deps` のようなコメント付きブロック終端も通常のブロック終端として扱い、`require` / `replace` / `exclude` ブロックのパースと更新に反映します。
`require "golang.org/x/text" "v0.14.0"` のような quoted module path / version も解析し、引用符を維持して更新します。
`go.mod` の単一行・ブロック形式の `require` 更新では、元の LF / CRLF 改行を維持します。

## mise のツールバージョン

mise（[jdx/mise](https://mise.jdx.dev)）の設定ファイルに書かれたツールバージョンも、他の言語と同じワークフロー（検出 → 解析 → 判定 → 書き込み）で更新します。バージョン一覧は `mise ls-remote <tool> --json` から取得するため、mise が対応するすべてのバックエンド（core / aqua / ubi / asdf / npm: / cargo: / go: / pipx: など）をそのまま扱えます。

### 対象ファイル

`mise.toml` / `.mise.toml` / `mise/config.toml` / `.mise/config.toml` / `.config/mise.toml` / `.config/mise/config.toml` / `.tool-versions`

`mise.local.toml`（個人のローカル上書き。通常 gitignore 対象）と `mise.<env>.toml`（環境別 overlay）は、意図しない環境だけを書き換えないよう対象外です。

### バージョン指定の扱い

| 記法 | 例 | 扱い |
|------|-----|------|
| 完全一致 | `node = "26.7.0"` | 最新版へ更新 |
| 前方一致 | `node = "26"` / `"26.7"` | セグメント数を保って更新（`26` → `27`、`26.7` → `26.8`） |
| 明示セレクタ | `go = "prefix:1.19"` | `prefix:` を保持して更新（`prefix:1.24`） |
| ベンダー付き | `java = "temurin-21.0.5"` | 同じベンダー内で更新（`temurin-21.0.9`） |
| inline table | `python = { version = "3.13", virtualenv = ".venv" }` | `version` だけ更新し他のオプションは保持 |
| テーブル形式 | `[tools.terraform]` + `version = "1.15.0"` | 同上 |
| 浮動指定 | `latest` / `lts` / `system` | 意味が変わらないため更新対象外 |
| 非バージョン | `ref:master` / `path:./shfmt` / `sub-2:lts` | 更新対象外 |
| 複数バージョン | `python = ["3.12", "3.13"]` | どれを更新すべきか決められないため対象外 |

`mise ls-remote java` が返す 3000 件超のうち大半は `temurin-` / `graalvm-community-` / `zulu-` などのベンダー接頭辞付きです。depup は現在の指定と同じ接頭辞の候補だけを対象にするため、`temurin-21` を使っているプロジェクトが `zulu-27` に書き換わることはありません。

`[tools]` セクションだけを書き換え、`[settings]` / `[env]` / `[tasks]` / `[alias]` に同名のキーがあっても変更しません。引用符の種別（`"` / `'`）、行末コメント、CRLF、`.tool-versions` の空白の並びはすべて保持します。

### mise とエイジフィルター

mise の `[settings] minimum_release_age` が **明示的に書かれている** 場合は、pnpm / bun の `minimumReleaseAge` と同じ「プロジェクトポリシー」として CLI の `--age` より優先します（mise 既定の 24h は「未設定」なので採用しません）。

```toml
[settings]
minimum_release_age = "7d"   # s / m（分）/ h / d / w / M / y
```

> **注意**: mise の `m` は **分**（humantime 準拠）で、depup CLI の `--age 1m`（1 か月）とは単位が違います。

`mise ls-remote` は既定で mise 側の `minimum_release_age` を適用して新しい版を隠しますが、depup は `--minimum-release-age 0` を渡して全件取得し、age 判定を depup 側に一本化します。`minimum_release_age_excludes` は depup では解釈しないため、設定されている場合は警告を表示します。

`--install` では `mise install` を実行し、解決済みの age を `MISE_MINIMUM_RELEASE_AGE` として渡します。

## エイジフィルター

`--age` オプションは、一定期間リリースされているバージョンのみに更新することで安定性を確保します。**デフォルトで 1 週間（`1w`）の age フィルターが適用されます**（明示的に上書きしない限り）：

```bash
# デフォルト — 暗黙の --age 1w
depup

# 2週間以上経過したバージョンのみに更新
depup --age 2w

# 10日以上経過したバージョンのみに更新
depup --age 10d

# 1ヶ月以上経過したバージョンのみに更新
depup --age 1m

# このランのみ age フィルターを無効化
depup --no-age
```

### グローバル設定

depup は初回実行時に `~/.config/depup/config.toml` をコメント付きの雛形で自動生成します。値を編集することでデフォルトをグローバルに変更できます：

```toml
# ~/.config/depup/config.toml

# depup の各ランに既定で適用する age フィルター。
# --age と同じ書式（Nd / Nw / Nm）。省略時は組み込みデフォルト（1w）を使用。
age = "1w"

# OSV.dev の脆弱性データベースで candidate を照会する（組み込みデフォルト: true）。
# 毎回チェックを外したい場合は false にする。
osv = true
```

**優先順位（高い順）:**
1. `--no-age`（age を完全に無効化）
2. `--age <DURATION>` CLI フラグ
3. `~/.config/depup/config.toml` の `age` 値
4. 組み込みデフォルト（`1w`）

### 適用される age の優先順位

プロジェクトに書かれた `minimumReleaseAge` は **プロジェクトポリシー** として最優先で扱われ、CLI / config の age 値を上書きします。完全な順序は以下の通り:

1. **プロジェクトの `minimumReleaseAge`**（最優先 — 複数ソースがあればより厳しい値）
2. CLI `--age <DURATION>`
3. `--no-age`（プロジェクトポリシーが無い場合のみ有効）
4. `~/.config/depup/config.toml` の `age` 値
5. 組み込みデフォルト `1w`

プロジェクトポリシーが CLI 指定を上書きする場合、depup は黄色で警告を出し、有効なソースを表示します:
```
⚠ --age ignored: project's minimumReleaseAge (14 days from pnpm-workspace.yaml) takes precedence
```

プロジェクトポリシーを回避したい場合は、プロジェクトファイル側で値を削除/上書きしてください。

### 対応する `minimumReleaseAge` のソース

**pnpm**（以下のいずれか — 最初に見つかった値）:
- `.npmrc`（`minimum-release-age=10d`）
- `pnpm-workspace.yaml`（`minimumReleaseAge: 14400` 分単位）
- `package.json`（`pnpm.settings.minimumReleaseAge`）

**bun**（`bunfig.toml`）:
```toml
[install]
minimumReleaseAge = 259200  # 秒単位（例: 3日）
```

**mise**（`mise.toml` などの `[settings]`）:
```toml
[settings]
minimum_release_age = "7d"  # s / m（分）/ h / d / w / M / y
```

複数のソースに値がある場合は、**より厳しい**（大きい）値を採用します。

### 推移的依存とエイジフィルター（Rust）

`--install` を指定すると、depup は install によって解決バージョンが変わった crate だけを対象に公開日を確認し、age ポリシーに違反するものを差し戻します：

```
Auditing transitive Rust dependencies [██████▒▒▒▒] 18/24 (Auditing hyper)
  . — 13 transitive dep(s) rolled back to satisfy --age:
    hyper 1.11.1 → 1.11.0
```

対象を「変わったもの」に限るのは、crates.io が 1 リクエスト/秒で応答するため、lockfile 全体（数百 crate）を舐めると画面に何も出ないまま数分止まってしまうからです。install 前に `Cargo.lock` が無かった場合は全エントリが新規扱いになるので、監査全体を 180 秒で打ち切り、残りは未検証として報告します（実行自体は失敗させません）。

### Swift とエイジフィルター

GitHub Tags API はタグのリリース日時を返しません。そのため Swift パッケージは `--age` 指定時もエイジフィルターの対象外として扱われます（更新対象に含まれます）。

## アップデートレベルの制限（`--max-change`）

`--max-change <LEVEL>` で、depup の bump を抑制できます：

```bash
# patch のみ許可（1.0.0 → 1.0.5 OK、1.0.0 → 1.1.0 NG）
depup --max-change patch

# patch + minor 許可（1.0.0 → 1.5.3 OK、1.0.0 → 2.0.0 NG）
depup --max-change minor

# デフォルト — 全 bump 許可（major 含む）
depup --max-change major
```

候補が上限を超える場合、その依存は `max-change=<LEVEL>` でスキップとして表示されます。Cargo の git tag 追従依存にも同じ上限を適用します。

### グローバル設定

`~/.config/depup/config.toml` で既定の上限を設定できます：

```toml
# 既定で patch + minor まで許可
max_change = "minor"
```

**優先順位（高い順）:**
1. `--max-change <LEVEL>` CLI フラグ
2. `~/.config/depup/config.toml` の `max_change` 値
3. 組み込みデフォルト（制限なし）

## 脆弱性チェック（OSV.dev）

depup は各 candidate バージョンを [OSV.dev](https://osv.dev/) の公開データベースに問い合わせ、既知の脆弱性があるバージョンを更新対象から除外します。**このチェックはデフォルトで有効**で、フラグの指定は不要です。age フィルターと組み合わせれば、安定して安全な次のバージョンへ自然にフォールバックします：

```bash
# OSV チェックはデフォルトで実行される
depup

# 明示的に有効化（グローバル設定の `osv = false` を上書き）
depup --osv

# このランのみ OSV チェックを無効化（グローバル設定とデフォルトを上書き）
depup --no-osv
```

- OSV.dev API はパブリックで、**認証トークンは不要**。
- Swift パッケージは対象外（OSV は GitHub リポジトリ URL でインデックスしており、depup が扱う Swift パッケージ名形式とは一致しないため）。
- mise のツールは対象外（バックエンドごとにバージョン体系も名前空間も異なり、単一の OSV ecosystem へ対応付けられないため）。
- API エラー時は更新を止めません。該当バージョンは candidate に残し、`--verbose` で警告を表示します。

### フォールバック例

candidate のバージョンに既知の脆弱性が見つかった場合、depup は自動的に次に古い安全なバージョンへフォールバックします。OSV チェックを通過した更新には `✓ OSV` マークが付きます：

```
$ depup --install --include-pinned
  ⚠ OSV: dompurify 3.4.8 vulnerable (GHSA-vxr8-fq34-vvx9)
./package.json (Node.js) — 9 updates, 41 skips
  @mui/icons-material   9.0.1 → 9.1.0 [minor] (2026/06/08 08:30) ✓ OSV
  @mui/material         9.0.1 → 9.1.0 [minor] (2026/06/08 08:29) ✓ OSV
  @tanstack/react-query 5.100.14 → 5.101.0 [minor] (2026/06/02 19:24) ✓ OSV
  next                  16.2.6 → 16.2.9 [patch] (2026/06/09 23:02) ✓ OSV
  openai                6.39.1 → 6.42.0 [minor] (2026/06/03 22:39) ✓ OSV
  react                 19.2.6 → 19.2.7 [patch] (2026/06/01 18:00) ✓ OSV
  react-dom             19.2.6 → 19.2.7 [patch] (2026/06/01 18:01) ✓ OSV
  @types/node           25.9.1 → 25.9.2 [patch] (2026/06/05 22:33) ✓ OSV 🔧
  @types/react          19.2.15 → 19.2.17 [patch] (2026/06/05 20:10) ✓ OSV 🔧

Errors:
  ✗ OSV check for dompurify: 3.4.8 vulnerable, falling back (GHSA-vxr8-fq34-vvx9)

Summary:
  9 package(s) updated (4 minor, 5 patch)
  41 package(s) skipped
```

`falling back` メッセージは設計どおりの正常な動作通知であり、exit code には影響しません。

### グローバル設定

チェックはデフォルトで有効です。常時オフにしたい場合は、自動生成された `~/.config/depup/config.toml` で明示的に無効化します：

```toml
# ~/.config/depup/config.toml

# 毎回 OSV チェックを行わない（組み込みデフォルトは true）
osv = false
```

**優先順位（高い順）:**
1. `--no-osv`（チェック無効化）
2. `--osv` CLI フラグ
3. `~/.config/depup/config.toml` の `osv` 値
4. 組み込みデフォルト（`true` — OSV チェック有効）

設定ファイルが存在しない場合、または `osv` キーが書かれていない場合は組み込みデフォルトが適用され、チェックが実行されます。

## uv マルウェアチェック（preview）

Python プロジェクトに対して `--install` が `uv sync` を呼び出す際、depup は環境変数 `UV_MALWARE_CHECK=1` を常時付与します。これは [uv の preview マルウェアチェック機能](https://astral.sh/blog/uv-audit)（`uv audit` コマンドとは別の、同時発表された独立 preview 機能）を有効化するもので、`uv sync` / `uv add` などの sync 操作のたびに、uv が現在 locked された resolution を OSV の MAL advisories と照合し、マルウェアが含まれていれば sync を中断して悪意あるパッケージの実行を防ぎます。

- 常時 ON。opt-in フラグは不要です。
- 機能未対応の古い uv バージョンではこの env var が単に作用しないため、強制 ON でも既存環境のビルドを壊しません。
- Astral 公式は preview 機能と位置づけており、将来挙動が変わる可能性があります。検査自体は uv 側で実行され、マルウェアにマッチすると uv が sync をエラー終了し、その終了コードがそのまま depup に伝搬します。

PHP プロジェクトを `--install` で処理するとき、depup は `composer install` ではなく `composer update` を実行します。`composer install` は既存 lock を再利用するため、depup が直前に `composer.json` へ書いた制約を反映できません。`composer update` で制約を再解決し、`composer.lock` を更新します。

## 出力

### 進捗表示

<p align="center">
  <img src="docs/images/scanning.png" alt="depup scanning">
</p>

### テキスト出力（デフォルト）

- `🔧` はdevDependenciesを示します
- リリース日は `(yyyy/mm/dd HH:MM)` 形式で表示
- 変更種別: `[major]`, `[minor]`, `[patch]`

### JSON出力

```bash
depup --json
```

```json
{
  "manifests": [
    {
      "path": "package.json",
      "language": "node",
      "updates": [
        {
          "type": "update",
          "dependency": {
            "name": "lodash",
            "version_spec": "^4.17.20"
          },
          "new_version": "4.17.21",
          "released_at": "2024-12-15T10:30:00Z"
        }
      ]
    }
  ]
}
```

### Diff出力

```bash
depup --diff
```

```diff
--- package.json
+++ package.json
@@ dependencies @@
-  "lodash": "^4.17.20"
+  "lodash": "^4.17.21"
```

## モノレポ対応

### `.depup` 設定ファイル

複数のサブディレクトリを持つモノレポプロジェクトでは、プロジェクトルートに `.depup` ファイルを作成して追加の処理対象ディレクトリを列挙できます：

```
# .depup
gui       # フロントエンドアプリ
api       # バックエンドAPI
shared    # 共有ライブラリ
```

ルートディレクトリから `depup` を実行すると、ルート自体と全ディレクトリの依存関係を一括更新します。バージョン情報はキャッシュされるため、同じパッケージは1回のみ取得されます。
`--install` 指定時は、更新された各マニフェストに最も近いモノレポ対象ディレクトリで package manager install を実行します。ネストしたアプリの依存更新は、リポジトリルートではなく各アプリのディレクトリで反映されます。

- `#` 以降はコメント（行頭・インライン両対応）
- 空行は無視
- パスは `.depup` ファイルの配置ディレクトリからの相対パス
- 絶対パス、親ディレクトリ（`..`）への移動、`.depup` の配置ディレクトリ外を指すシンボリックリンクは拒否
- 存在しないディレクトリは警告してスキップ
- ルートディレクトリは常にスキャン対象に含まれる

### pnpmワークスペース

depupは `pnpm-workspace.yaml` を検出し、全てのワークスペースパッケージを処理します。`packages` 配列は block-style (`- 'packages/*'`) と flow-style (`packages: ['packages/*', 'apps/*']`) の両方に対応し、否定パターン (`!packages/legacy`) も扱えます。

### Cargoワークスペース

`[workspace] members`（`crates/*` のような glob パターンを含む）を展開し、`[workspace] exclude` に挙げられたメンバーは除外します。

### Goワークスペース

`go.work` の `use` ディレクティブ（単一行形式と `use ( ... )` ブロック形式の両方）を展開し、各メンバーモジュールの `go.mod` を処理します。展開しないと、ルートに `go.mod` が無い構成では全メンバーの依存が古いままでも「更新なし」と報告してしまいます。

### Gradleマルチプロジェクト

`settings.gradle` / `settings.gradle.kts` の `include ':app', ':core'`（Groovy 形式と Kotlin DSL 形式の両方）を展開し、各サブプロジェクトの `build.gradle` / `build.gradle.kts` と `buildSrc/` を処理します。依存宣言の大半はサブプロジェクト側にあるため、ルートのビルドファイルだけを見ていると取りこぼします。

展開先のパスには `.depup` と同じ安全性検査が適用されます。絶対パス、`..` による親ディレクトリ参照、プロジェクト外へ解決される symlink は拒否されます。

### Tauriプロジェクト

depupはTauriプロジェクトの `src-tauri/Cargo.toml` を自動検出します。

#### Tauriバージョン同期

Tauriプロジェクトでは、npmの `@tauri-apps/api` とRustの `tauri` クレートのメジャー/マイナーバージョンが一致している必要があります。depupは自動的にこれらのバージョンを同期し、ビルドエラーを防止します。

```
# エラー例（バージョン不一致）
Found version mismatched Tauri packages:
  tauri (v2.10.1) : @tauri-apps/api (v2.9.1)

# depupが自動的にバージョンを同期
@tauri-apps/api: 2.9.1 → 2.10.0
tauri: 2.9.0 → 2.10.1
```

両方のパッケージが同じメジャー.マイナーバージョン（例：2.10.x）になるよう自動調整されます。

## ビルド

```bash
# デバッグビルド
cargo build

# リリースビルド
cargo build --release

# テスト実行
cargo test

# ローカルインストール
cargo install --path .
```

## コントリビュート

コントリビュートを歓迎します！お気軽にプルリクエストをお送りください。

## ライセンス

[MIT](LICENSE)
