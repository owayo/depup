<p align="center">
  <img src="docs/images/app.png" width="128" alt="depup">
</p>

<h1 align="center">depup</h1>

<p align="center">
  複数言語の依存関係をまとめて更新する CLI ツール
</p>

<h3 align="center">対応プラットフォーム</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&amp;logoColor=black" alt="Linux">
  <img src="https://img.shields.io/badge/macOS-000000?logo=apple&amp;logoColor=white" alt="macOS">
  <img src="https://img.shields.io/badge/Windows-0078D6" alt="Windows">
  <br>
  <a href="https://github.com/owayo/depup/actions/workflows/release.yml"><img src="https://github.com/owayo/depup/actions/workflows/release.yml/badge.svg?branch=main" alt="Release"></a>
  <a href="https://github.com/owayo/depup/actions/workflows/ci.yml"><img src="https://github.com/owayo/depup/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/owayo/depup/releases"><img src="https://img.shields.io/github/v/release/owayo/depup" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

<h3 align="center">対応言語</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Node.js-339933?logo=nodedotjs&amp;logoColor=white" alt="Node.js">
  <img src="https://img.shields.io/badge/Python-3776AB?logo=python&amp;logoColor=white" alt="Python">
  <img src="https://img.shields.io/badge/Rust-000000?logo=rust&amp;logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/Go-00ADD8?logo=go&amp;logoColor=white" alt="Go">
  <img src="https://img.shields.io/badge/Ruby-CC342D?logo=ruby&amp;logoColor=white" alt="Ruby">
  <img src="https://img.shields.io/badge/PHP-777BB4?logo=php&amp;logoColor=white" alt="PHP">
  <img src="https://img.shields.io/badge/Java-ED8B00?logo=openjdk&amp;logoColor=white" alt="Java">
  <img src="https://img.shields.io/badge/Swift-F05138?logo=swift&amp;logoColor=white" alt="Swift">
  <img src="docs/images/badge-mise.svg" alt="mise">
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
      <img src="docs/images/output_python.png" width="400" alt="depup の Python 出力例">
    </td>
    <td align="center">
      <strong>Tauri (package.json + Cargo.toml)</strong><br>
      <img src="docs/images/output_tauri.png" width="400" alt="depup の Tauri 出力例">
    </td>
  </tr>
</table>

## 特徴

- **複数言語対応**: Node.js、Python、Rust、Go、Ruby、PHP、Java、Swift
- **mise 対応**: `mise.toml` / `.tool-versions` のツールバージョンも同じワークフローで更新
- **マニフェスト更新**: マニフェストファイル内のバージョン指定を直接更新
- **範囲指定の維持**: バージョン範囲の形式（`^`、`~`、`>=`）を保ったまま、上限を壊さずに更新
- **固定バージョン検出**: 意図的に固定されたバージョンはデフォルトでスキップ
- **age フィルター**: 公開から N 日（または N 週）以上経過したバージョンにのみ更新
- **pnpm 連携**: pnpm 設定の `minimumReleaseAge` を自動適用
- **Bun Catalogs 対応**: `package.json` の Bun `catalog` / `catalogs` 定義を更新
- **モノレポ対応**: `.depup`、Cargo / pnpm / Go のワークスペース、Gradle マルチプロジェクト、入れ子のパッケージごとの install、Tauri プロジェクト
- **リリース日表示**: 各バージョンのリリース日時を表示
- **複数出力形式**: テキスト（カラー）、JSON、diff

## 対応言語

| 言語 | マニフェスト | レジストリ | ロックファイル |
|------|-------------|----------|---------------|
| <img src="https://img.shields.io/badge/-339933?logo=nodedotjs&logoColor=white" height="16"> Node.js | package.json（Bun catalogs を含む） | npm | package-lock.json、pnpm-lock.yaml、yarn.lock、bun.lock、bun.lockb |
| <img src="https://img.shields.io/badge/-3776AB?logo=python&logoColor=white" height="16"> Python | pyproject.toml | PyPI | uv.lock、requirements.lock、poetry.lock |
| <img src="https://img.shields.io/badge/-000000?logo=rust&logoColor=white" height="16"> Rust | Cargo.toml | crates.io | Cargo.lock |
| <img src="https://img.shields.io/badge/-00ADD8?logo=go&logoColor=white" height="16"> Go | go.mod（go.work のメンバーも自動検出） | Go Proxy | go.sum |
| <img src="https://img.shields.io/badge/-CC342D?logo=ruby&logoColor=white" height="16"> Ruby | Gemfile | RubyGems | Gemfile.lock |
| <img src="https://img.shields.io/badge/-777BB4?logo=php&logoColor=white" height="16"> PHP | composer.json | Packagist | composer.lock |
| <img src="https://img.shields.io/badge/-ED8B00?logo=openjdk&logoColor=white" height="16"> Java | build.gradle、build.gradle.kts、gradle/*.versions.toml（settings.gradle のサブプロジェクトも自動検出） | Maven Central | gradle.lockfile |
| <img src="https://img.shields.io/badge/-F05138?logo=swift&logoColor=white" height="16"> Swift | Package.swift | GitHub Tags | Package.resolved |
| <img src="docs/images/badge-mise-icon.svg" height="16"> mise | mise.toml、.mise.toml、.config/mise/config.toml、.tool-versions など | `mise ls-remote` | mise.lock |

## 動作要件

- **OS**: macOS、Linux、Windows
- **Rust**: 1.85 以上（ソースからビルドする場合）
- **mise**: バージョン一覧を `mise ls-remote` から取得するため、mise のツールバージョンを更新する場合にのみ必要です。mise がない環境では mise の設定ファイルをスキップし、警告を 1 回表示して他の言語の更新を続けます。

## インストール

### Homebrew (macOS/Linux)

```bash
brew install owayo/depup/depup
```

### winget (Windows)

```powershell
winget install owayo.depup
```

### ソースから

```bash
git clone https://github.com/owayo/depup.git
cd depup
cargo install --path .
```

### GitHub リリースから

[Releases](https://github.com/owayo/depup/releases) から最新のバイナリをダウンロードできます。

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

`depup-x86_64-pc-windows-msvc.zip` を [Releases](https://github.com/owayo/depup/releases) からダウンロードし、展開して PATH に追加してください。

> `winget install owayo.depup` を使えば PATH 登録まで自動で行われるため、手動ダウンロードが必要なのは winget を使わない場合だけです。winget でインストールした直後は、PATH の変更を反映させるためにターミナルを開き直してください。

## クイックスタート

```bash
# すべての依存関係の更新内容を確認（ドライラン）
depup -n

# Node.js の依存関係のみ更新
depup --node

# 公開から 2 週間以上経過したバージョンにのみ更新
depup --age 2w

# diff を表示して更新
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
| `--cd <DIR>` | `-C` | 指定したディレクトリに移動してから実行 |
| `--dry-run` | `-n` | ファイルを変更せずに更新内容を表示 |
| `--verbose` | | 詳細出力を有効化 |
| `--quiet` | `-q` | 最小限の出力 |
| `--node` | | Node.js の依存関係のみ更新 |
| `--python` | | Python の依存関係のみ更新 |
| `--rust` | | Rust の依存関係のみ更新 |
| `--go` | | Go の依存関係のみ更新 |
| `--ruby` | | Ruby の依存関係のみ更新 |
| `--php` | | PHP の依存関係のみ更新 |
| `--java` | | Java の依存関係のみ更新 |
| `--swift` | | Swift の依存関係のみ更新 |
| `--mise` | | mise（mise.toml / .tool-versions）のツールバージョンのみ更新 |
| `--exclude <PKG>` | | 指定したパッケージを除外（複数指定可） |
| `--only <PKG>` | | 指定したパッケージのみ更新（複数指定可） |
| `--include-pinned` | | 固定バージョンも更新対象に含める |
| `--age <DURATION>` | | 公開からの最小経過期間（例: `2w`、`10d`、`1m`）。グローバル設定を上書き |
| `--no-age` | | この実行に限り age フィルターを無効化（グローバル設定とデフォルトを上書き） |
| `--osv` | | 更新候補を OSV.dev の脆弱性データベースで照会し、既知の脆弱性があるバージョンをスキップ（デフォルトで有効） |
| `--no-osv` | | この実行に限り OSV の脆弱性チェックを無効化（グローバル設定とデフォルトを上書き） |
| `--max-change <LEVEL>` | | 許容する更新の上限。`patch`（patch のみ）/ `minor`（patch と minor）/ `major`（すべて許可、デフォルト） |
| `--json` | | JSON 形式で出力 |
| `--diff` | | 変更内容を diff 形式で表示 |
| `--install` | | 更新後にパッケージマネージャーの install を実行 |
| `--version` | `-V` | バージョンを表示 |
| `--help` | `-h` | ヘルプを表示 |

### 使用例

```bash
# すべての更新内容をプレビュー
depup -n

# lodash と typescript のみ更新
depup --only lodash --only typescript

# react を更新対象から除外
depup --exclude react

# 同じパッケージを exclude に指定しても only が優先される
depup --only lodash --exclude lodash

# 公開から 2 週間以上経過したバージョンにのみ更新
depup --age 2w

# Python と Rust のみ更新
depup --python --rust

# Java（Gradle）の依存関係のみ更新
depup --java

# Swift（Package.swift）の依存関係のみ更新
depup --swift

# mise（mise.toml / .tool-versions）のツールバージョンのみ更新
depup --mise

# CI/CD 向けに JSON で出力
depup --json

# 更新後に npm install などを実行
depup --node --install

# 別のディレクトリで実行
depup --cd ./projects/myapp -n
```

## バージョン処理

### 固定バージョン（デフォルトで除外）

バージョンを固定した依存は意図的に固定されているものとみなし、デフォルトで更新対象から外します。

| 言語 | 指定例 | 更新 |
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
| Java | Gradle の固定バージョン | ❌ |
| Java | Gradle の strict 記法（`1.2.3!!`） | ❌ |
| Java | Maven の Hard requirement（`[1.0]`） | ❌ |
| Swift | `exact: "1.2.3"` | ❌ |
| Swift | `from: "1.2.3"`, `.upToNextMinor` | ✅ |

`--include-pinned` で固定バージョンも更新対象にできます。

> **注意**: `// pinned` コメントのない Go の依存関係は、`--include-pinned` の有無にかかわらず常に更新対象です。これは `go.mod` に `^` や `~` のような範囲指定がなく、完全なバージョンしか書けないためです。Go のバージョン指定は、もともとすべて固定と同じ状態です。
>
> **注意**: Go の `exclude` ディレクティブは、記述自体は書き換えずに、更新候補からの除外に反映します。上流モジュールの最新の `go.mod` が `retract` したバージョン（単一のバージョン、閉区間のどちらでも）も候補から除外します。タグ付きのバージョンがないモジュールでは Go Proxy の `@latest` へフォールバックします。`.info` の `Time` が省略されている場合は Unix エポックを公開日時として扱うため、公開日が不明なバージョンが `--age` で永久に除外されることはありません。
>
> **注意**: `gem "pg", ">= 0.18", "< 2.0"` のような Gemfile の複合制約も解析・更新できます。進めるのは包含下限（`>= 0.18` のように値そのものを含む下限）だけで、元の引数の個数・順序・引用符の種類・空白・括弧付きの呼び出し形式・行末の条件修飾子を保ったまま書き戻します。比較の基準は記述順に関係なく包含下限なので、`gem "pg", "< 2.0", ">= 0.18"` でも `0.18` を基準に判定します。書き換え後の制約を元の引数の個数へ分割できない場合（引数自体がカンマを含む場合など）は、安全でない書き換えはせずにエラーとして報告します。`gem "rack", "!= 2.2.4"` のような除外制約は、一部だけを更新すると意味が変わるため、判定の段階でスキップします。
>
> **注意**: Bundler の `git_source(:name) { ... }` で登録したカスタムの git source ショートハンド（例: `gem 'rails', stash: 'forks/rails'`）も、組み込みの `git:` / `github:` と同じくレジストリ外の依存として更新対象から除外します。
>
> **注意**: バージョンを書かずに `git:` / `github:` / `bitbucket:` / `gist:` / `path:` / `source:` を指定した Gemfile の依存は、RubyGems のレジストリ制約に変換せずスキップします。同じ形式でもバージョンが明示されていれば、Bundler が gemspec のバージョンを検証するための制約として、source オプションを保持したまま解析・更新できます。オプションキーは Ruby の 2 通りの綴り（`git: '...'` と hash-rocket 形式の `:git => '...'`）をどちらも認識します。`git ... do` / `github ... do` / `path ... do` / `source ... do` ブロック内の gem も同じ理由でスキップし、`platforms` / `install_if` のような通常のブロックは従来どおり処理します。引数が次の行へ続く宣言（`gem "devise",`）は、その行だけではバージョンを決められないため、「バージョンなしのレジストリ依存」として報告せずにスキップします。行単位の `group:` / `groups:` オプションは、開発依存かどうかの判定に使います。
>
> **注意**: Gemfile の依存宣言は、一般的な Ruby DSL 形式（`gem "rack", "~> 3.0"`）と括弧付きのメソッド呼び出し形式（`gem("rack", "~> 3.0")`）のどちらも解析・更新できます。更新時は元の呼び出し形式を保持します。同じ gem が複数箇所（例: トップレベルと `group :test` ブロックの両方）で宣言されている場合は、書き込み先が曖昧なので書き換えを拒否します。
>
> **注意**: `alias = { package = "actual-crate", version = "1" }` のような Cargo のリネーム依存は、実際のパッケージ名でバージョンを取得し、マニフェスト上のキーへ書き戻します。`--only` / `--exclude` はどちらの名前でも指定できます。
>
> **注意**: `--only` を指定した場合は `--exclude` より優先されます。明示的に許可したパッケージは、同じ名前が広い除外リストに含まれていても更新対象に残ります。
>
> **注意**: Composer のプラットフォームパッケージ（`php`、`hhvm`、`ext-*`、`lib-*`、Composer API パッケージなど）は更新対象から除外します。
>
> **注意**: Composer と Packagist は、`composer/semver` の `VersionParser` に従って 1〜4 セグメントの数値バージョンを有効とみなします。depup も `1.2.3.4`、`^1.0.0.0`、`~3.4.5.6`、`1.0.0.*` のような 4 セグメントまでのバージョンを解析・更新でき、5 セグメント以上は無効として除外します。
>
> **注意**: Composer の修飾子（modifier）は区切り文字を省略でき、`.` / `_` も区切りに使えます（`composer/semver` の正規表現は `[._-]?`）。そのため depup は、`5.0.0alpha3` / `1.0.0.RC1` / `1.0.0_beta1` をプレリリース、`2.2.1p1` / `2.2.1pl1` / `2.2.1patch1` を元のバージョンより**新しい**パッチ版（patch alias）として扱います。どちらの形式も現在の Packagist に実在します（`nikic/php-parser` の `5.0.0beta1`、`laminas/laminas-diactoros` のセキュリティパッチ `2.2.1p2`）。
>
> **注意**: Gradle の `-SNAPSHOT` / `.SNAPSHOT` は更新対象から除外します。SNAPSHOT は依存を解決するたびに最新のタイムスタンプ付きビルドを取り直す「動く参照」なので、固定のリリース版へ書き換えると、ビルドが使うものが気づかないうちに変わってしまいます。`.Final` / `.RELEASE` / `-jre` / `-SP1` のような安定版の qualifier は、従来どおり更新します。
>
> **注意**: Gradle の `resolutionStrategy { force ... }` / `constraints { }` / `dependencySubstitution { }` の中に書かれた座標は、依存として報告しません。これらは他の場所で宣言済みのバージョンを再掲するもので、別々の宣言として扱うと同じ座標が重複し、書き込み先が曖昧になって更新が拒否されてしまうためです。
>
> **注意**: Go の `+incompatible` 付きバージョンは、`go` コマンドと同じ規則で除外します。SemVer の順序で最初の `+incompatible` に達したとき、直前の compatible なバージョンが実際に `go.mod` を持っていれば、それ以降の `+incompatible` をすべて候補から外します。この処理がないと、`github.com/libp2p/go-libp2p` が `v0.49.0` から 2018 年の `v6.0.23+incompatible` へ「更新」され、しかもビルドは通ってしまいます。
>
> **注意**: PyPI 以外をデフォルトのインデックスにしている `pyproject.toml`（Poetry の `priority = "primary"` / `"default"` の source、uv の `[[tool.uv.index]] default = true` や `[tool.uv] index-url`、PDM による `pypi` の上書き）では、警告を出したうえで全依存をスキップします。depup は PyPI しか参照しないため、更新すると非公開パッケージが同名の公開パッケージに置き換わってしまいます。

### 範囲形式の維持

depup は元のバージョン範囲の形式を維持します。

```
"^1.2.3" → "^2.0.0"  （キャレット維持）
"~1.2.3" → "~1.3.0"  （チルダ維持）
"~1.2"   → "~1.9"    （チルダのセグメント数を維持。増やすと許容幅が狭まる）
"~1"     → "~2"      （1 セグメントのチルダはメジャー単位の幅を維持）
"~1.2 <2.0.0" → "~1.9 <2.0.0" （comparator set に埋め込まれたチルダもセグメント数を維持）
"~1, <5.0" → "~4, <5.0" （Cargo の複数要件でもチルダの幅を維持）
"~> 7.0" → "~> 8.1"  （RubyGems の悲観的演算子。セグメント数を維持）
">=1.0.0" → ">=2.0.0" （範囲維持）
"requests (>=2.28,<3); python_version < '3.12'" → "requests (>=2.31,<3); python_version < '3.12'" （PEP 508 の括弧とマーカーを維持）
"coverage [toml] >=7,<8" → "coverage [toml] >=7.6,<8" （PEP 508 extras の空白を維持）
"'paramiko>=3.5.0,<4.0.0,'" → "'paramiko>=3.9.1,<4.0.0,'" （PEP 508 の末尾カンマを維持）
"'paramiko>=3.5.0,<4.0.0'" → "'paramiko>=3.9.1,<4.0.0'" （TOML リテラル文字列の引用符を維持）
"1.x" → "2.x" （ワイルドカード形式を維持）
"1.2.x - 2.3.x" → "1.9.x - 2.3.x" （npm のハイフンレンジ。端点が x-range）
"1.x.x" → "2.x.x" （複数のワイルドカード位置を維持）
"1.2.*" → "1.3.*" （ワイルドカード形式を維持）
"v1.*" → "v2.*" （先頭の `v` を維持）
"V1.*" → "V2.*" （Composer の大文字 `V` を維持）
"^1.x" → "^2.x" （npm のキャレット + x-range。演算子を維持）
"~1.2.x" → "~2.3.x" （npm のチルダ + x-range。演算子を維持）
"=1.x" → "=2.x" （npm の等号 + x-range。演算子を維持）
"=1.2" → "=2.3" （npm の partial comparator。演算子を維持）
"5.3.+" → "5.4.+" （Gradle のプレフィックスを維持）
"5.3.+!!" → "6.1.+!!" （Gradle strict の動的プレフィックスを維持）
"1.2.3!!" → "2.0.0!!" （Gradle strict を維持）
"[1.7, 1.8[!!" → "[1.7.36, 1.8[!!" （prefer なしの Gradle strict 範囲）
"[1.0]" → "[2.0]" （Maven の Hard requirement を維持）
"[1.2.3.Final]" → "[1.3.0]" （qualifier 付きの Maven Hard requirement）
group = "com.google.guava", name = "guava", version = "32.1.2-jre" → version = "33.4.0-jre" （Gradle Kotlin の map 記法）
junit = "junit:junit:4.13.2" → "junit:junit:4.13.3" （Gradle version catalog の library）
guava = "32.1.2-jre" → "33.4.0-jre" （Gradle version catalog の version 参照）
prefer("1.7.25") → prefer("1.7.36") （Gradle rich version の strict 範囲内の prefer）
"org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.25" → "org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.36" （Gradle strict 範囲の prefer 短縮記法）
"group:name:1.0.0:classifier@zip" → "group:name:1.1.0:classifier@zip" （Gradle の classifier / extension を維持）
```

`"*"`、npm の dist-tag（`"latest"` など）、Gradle の動的指定（`"latest.release"`、`"latest.integration"`、`"latest.milestone"`、ユーザー定義の `latest.<status>` など）のような完全な浮動指定は、固定バージョンに変わってしまわないよう更新対象から除外します。Composer の `*.*` / `v*` / `V*` / `x.x` のように数字を含まない複数セグメントのワイルドカードと、Java / Gradle の `[,]` / `(,)` のように下限も上限も空の Maven 形式の範囲も同様に除外します（実際には何も変わらない更新を毎回報告したり、「常に古い」と誤判定したりする原因になるため）。`1.x.3` や `^x.0.0` のように、ワイルドカード文字（`x` / `X` / `*`）の後ろに数値セグメントが続く形式は、node-semver や Rust の semver クレートでは無効な x-range なので、解析の時点で除外します。

npm のバージョン文字列は、更新対象として解析する前に、プレリリース識別子とビルドメタデータ識別子を検証します。アンダースコアを含む識別子（`1.2.3-rc_1`）、空の識別子を含む形式（`1.2.3-alpha..1`）、先頭がゼロの数値プレリリース識別子（`1.2.3-01`）は、不正な package.json の制約へ正規化せずにスキップします。

先頭ゼロの検証は、ビルドメタデータを切り落としてから行います。SemVer ではビルド識別子にハイフンや先頭ゼロを含められるため、`1.0.0+2024-01` や `1.2.3+00` のようなバージョンは有効なものとして更新対象に残します（検証するのはプレリリース部分だけです）。

npm の partial comparator（`=1.2` や `=1`）は、固定バージョンではなく、node-semver の部分バージョン規則に従うものとして扱います。depup は `=` 演算子と、書かれているセグメント数を保って更新します（`=1.2` → `=2.3`、`=1` → `=2`）。

更新候補の順序は、エコシステムごとの規則で比較します。Node.js / Rust / Go / Swift は SemVer を使い、数値だけのプレリリース（`1.0.0-1`）もプレリリースとして扱います。優先順位の比較ではビルドメタデータを無視するため、`1.1.3` と `1.1.3+spec-1.1.0` の違いだけでは更新しません。Python は PEP 440 の正規化、Ruby は RubyGems のセグメント順（英字またはハイフンを含むバージョンはプレリリース）、Java は Gradle 公式のバージョン順序に従います。Composer の patch alias（`-p1` / `-pl1` / `-patch1`）は、対応する安定版より新しいものとして扱います。数値セグメントは桁数の上限なしで比較するため、巨大な数値でも桁あふれしません。

Node.js は node-semver 互換の従来形式 `~>1.2.3` も受け付け、更新後も `~>` を保持します。Composer は明示的な等価演算子（`=1.2.3` / `==1.2.3`）を保持したまま更新します。除外指定の `<>1.2.3` は解析しますが、安全のため自動では書き換えません。

`strictly` / `require` / `prefer` / `reject` を使う Gradle の rich version 宣言は、`implementation("org.slf4j:slf4j-api") { version { ... } }` のような依存ブロック内でも解析します。文字列記法でも、`group:name:1.2.3!!`、`group:name:5.3.+!!`、`group:name:[1.7, 1.8[!!`、prefer 付き strict 範囲の `group:name:[1.7, 1.8[!!1.7.25` のような短縮記法で書かれた固定値・動的プレフィックス・範囲を解析できます。`strictly` または `require` で範囲を、`prefer` で優先するバージョンを指定している場合、depup は範囲を上限制約として維持したまま `prefer` の値を更新します。`reject` に列挙されたバージョンは更新候補から除外し、`2.+` のような動的な reject や `[1.5,1.9)` のような範囲の reject も考慮します。

Gradle の宣言ラッパー `platform(...)` / `enforcedPlatform(...)` / `testFixtures(...)` にも対応しています。`implementation platform('com.google.cloud:libraries-bom:26.1.0')` や `testImplementation(platform("org.junit:junit-bom:5.10.0"))` のような BOM 宣言も解析・更新でき、開発依存かどうかはラッパーの外側にある configuration 名（`implementation` / `testImplementation` など）で判定します。`ext.<name> = '...'` / `project.ext.<name> = "..."` のドット代入も `ext { ... }` ブロックと同じく変数として解決し、`${Versions.retrofit}` のような修飾付きの参照は最後のセグメントで解決します。同じ変数名が異なる値で複数定義されている場合は、別オブジェクトの値を拾って誤更新しないよう、更新しません。

`gradle/*.versions.toml` にある Gradle version catalog は、Java のマニフェストとして検出します。depup は `[libraries]` の `alias = "group:name:version"`、`module = "group:name"`、`group` / `name` / `version`、`version.ref` を解析し、参照先の `[versions]` もその場で更新します。`strictly` / `require` / `prefer` / `reject` / `rejectAll` を含む rich version table は、Gradle のビルドファイルと同じ規則で候補を選びます。`[plugins]` は Gradle のプラグイン ID で、Maven Central の座標とは一致しないため更新対象から除外します。

Python の互換リリース指定（compatible release、`~=`）は PEP 440 に従います。`~=1.2`（= `>=1.2,<2.0`）と `~=1.2.3`（= `>=1.2.3,<1.3.0`）は上限が明示された範囲として扱い、互換の範囲内で更新します（`~=1.2.3` は 1.2 系、`~=1.2` は 1.x 系に留まり、メジャー・マイナーをまたぎません）。書き換えでは元のセグメント数を維持します（`~=1.2` → `~=1.9`。`~=1.9.0` にすると上限が `<1.10.0` に変わってしまうため）。単一セグメントの `~=1` は無効な形式なのでスキップします。

PEP 440 の前方一致（prefix matching）は、`==1.2.*` / `!=1.2.*` のようにリリース部分に `==` / `!=` を付けた指定でのみ受け付けます。`>=1.0.*`、`~=1.0.*`、`==1.0a1.*`、`==1.0.post1.*`、`==1.0+local.*` のような無効な形式は、解析の時点でスキップします。任意一致（arbitrary equality）の `===1.0.*` は、前方一致ではなく固定指定として扱います。

JVM 系の milestone 版はプレリリースとして扱い、安定版の更新候補から除外します。対象は `4.0.0-M1`、旧 Spring Boot のドット区切り `2.0.0.M1`、省略しない綴りの `-milestone1` です。この扱いがないと、`assertj-core 3.24.2` が `4.0.0-M1` へ、`junit-bom 5.10.0` が `5.13.0-M3` へ、`spring-core 5.3.23` が `7.0.0-M6` へ更新されてしまいます。判定は「`m` の直後が数字」のトークンに限るため、JVM の安定版の qualifier（`.Final`、`-jre`、`-android`、`.RELEASE`、`.GA`、`-SP1`）や `-macos1` のような識別子を誤判定しません。他のプレリリースと同じく、現在のバージョンが milestone の場合は候補に milestone を残すため、次の milestone へ進めます。

PEP 440 のプレリリースは、区切り文字なしで書かれた場合（例: `2.0.0rc1`、`1.0rc1`、`1.0.0a1`）でも検出し、デフォルトで除外します。そのため、安定版を使っている依存がリリース候補（rc）へ誤って更新されることはありません。ポストリリース（`1.0.post1`）は対応する安定版より新しいものとして比較し、エポック（`1!2.3`）は比較で最優先します。プレリリースに付いたポストリリース（`1.0a1.post1`）も元のプレリリースより新しいものとして扱う（`1.0a1 < 1.0a1.post1 < 1.0`）ため、アルファ版を追っているユーザーもポストリリースの修正を取りこぼしません。

PEP 440 の local version（`+` 以降のラベル）は、SemVer のビルドメタデータとは別物として、Python 固有の意味で扱います。固定指定と除外指定では local ラベルを保持します（`==1.0+cu121`、`!=1.0+local1`）。候補の比較では、local version を同じ公開バージョン（public version）より新しいものとして扱い（`1.0+local > 1.0`）、local 部分どうしも PEP 440 に従って比較します（`1.0+1 > 1.0+abc`、`1.0+abc.2 > 1.0+abc.1`）。PEP 440 が local ラベルを許していない順序比較・互換リリースの指定（`>=1.0+local`、`~=1.0+local`、`>=1.0+local,<2.0`）はスキップします。

Poetry の `[tool.poetry.dependencies]` では、演算子のないバージョン文字列（`requests = "2.28.0"`）は完全一致の固定指定です（Poetry 公式ドキュメントの "Exact requirements"。`==2.28.0` と同じ意味）。depup は文字列形式と inline table（`{ version = "1.26.0", optional = true }`）のどちらでも、これを完全一致で固定された依存として解析します。そのため明示的な `==2.28.0` と同じく固定バージョンとして一覧に表示され、`--include-pinned` を付ければ更新できます。更新時は演算子を付けずに新しいバージョンへ書き換えます（`4.2.1` → `5.0.0`）。演算子なしを完全一致とみなすのは Poetry の設定内だけで、pip や PEP 508 の依存指定では演算子が必須なので、演算子のないバージョンは受け付けません。

### 範囲制約

depup は上限付きの範囲制約を守ります。上限は排他的（`<` など）と包含的（`<=` など）のどちらにも対応します。

```
">=3.5.0,<4.0.0"   → ">=3.9.1,<4.0.0"
">=1.0,<=2.0"      → ">=2.0,<=2.0"
"4.0.0..<5.0.0"    → "4.99.0..<5.0.0"
"4.0.0...4.9.9"    → "4.9.9...4.9.9"
"1.2.0 - 2.0.0"    → "1.9.3 - 2.0.0" （npm のハイフンレンジ）
"1.0 - 2.0"        → "2.0.9 - 2.0" （npm / Composer の部分指定の上限は `<2.1` に展開）
"[1.0,2.0)"        → "[1.9.3,2.0)" （Maven 形式）
"[1.0,2.0]"        → "[2.0,2.0]" （Maven 形式）
"[1.0,2.0.Final)"  → "[1.9.3,2.0.Final)" （Maven の qualifier）
"[1.0,2.0-beta1-SNAPSHOT)" → "[1.9.3,2.0-beta1-SNAPSHOT)" （複数に区切られた Maven の qualifier）
"[1.0,2.0["        → "[1.9.3,2.0[" （Maven の排他上限の別表記 `[`）
"<4.0.0"           → スキップ（上限のみの制約）
">1.0.0"           → スキップ（排他的な下限）
"]1.0,2.0["        → スキップ（Maven の排他的な下限）
```

npm / Composer のハイフンレンジで右辺が `1.0 - 2.0` のような部分指定のときは、ワイルドカードを展開した排他的な上限として解釈します。そのため `2.0.x` は更新候補に含まれ、`2.1.0` 以降は除外されます。

node-semver の `HYPHENRANGE` は両端に `XRANGEPLAIN` を許すため、`1.x - 2.x` のような x-range の端点も有効な指定として解析・更新します。ただし depup が他の箇所で除外している端点（`1.x.3` のようにワイルドカードの後ろに数値が続く形式や、数字を含まない `*`）は、ここでも除外します。

Composer は `~>` 演算子を受け付けない（`Invalid operator "~>"`）ため、PHP では `~>` をスキップし、Composer が読めない制約を書き戻さないようにしています。node-semver は `~>` を有効とするので、Node.js では従来どおり受け付けます。

Cargo の `registry-index = "..."` 付き依存は、`registry = "..."` と同じく crates.io 以外のレジストリを指すため、更新対象から除外します。

依存関係に上限付きの範囲（例: `>=3.5.0,<4.0.0`、`>=1.0,<=2.0`、`4.0.0...4.9.9`）がある場合、depup は次のように動作します。

- 上限を超えるバージョンは**提案しません**。
- 包含的な上限（`<=`、`...`）では、上限値そのものも候補に含めます。
- マニフェストファイル内の元の制約の形を**維持**します。
- 範囲内で互換性のある最新バージョンに合わせて、**下限側だけを更新**します。

安全に書き換えられない制約は、部分的に更新せずにスキップします。たとえば npm / Composer の OR 制約（`^1 || ^2`）と Composer の後方互換表記である単一パイプ（`^1 | ^2`）、`!=` を含む除外制約（`!=1.2.3`、`>=1.0, !=1.5.0, <2.0`）、上限のみの制約（`<4.0.0`、`<=2.0`）、排他的な下限の制約（`>1.0.0`）、下限のない Maven 形式の範囲（`(,2.0]`）、排他的な下限を持つ Maven の範囲（`]1.0,2.0[`）が該当します。Composer（composer/semver）は not-equal を `<>` とも書けるため、`>=1.0 <>1.5.0 <2.0` / `>=1.0,<>1.5.0,<2.0` のような `<>` による除外制約も、`!=` の場合と同じくスキップします。

JSON マニフェストでは、depup が解析対象とする依存セクションだけを書き換えます。`package.json` の `overrides` や、`composer.json` の `replace` / `provide` / `conflict` などは変更しません。

同じ依存キーがマニフェスト内で複数回宣言されている場合や、複数の Gradle 依存が 1 つのバージョン変数または version catalog の `version.ref` を共有している場合、depup は曖昧な書き込みを拒否します。スキップ対象や固定された別の宣言を、巻き添えで書き換えないためです。

Bun のワークスペースでは、ルートの `package.json` にあるトップレベルの `catalog` / `catalogs` と、`workspaces.catalog` / `workspaces.catalogs` を解析・更新します。ワークスペース内パッケージの `"react": "catalog:"` や `"jest": "catalog:testing"` のような参照は `catalog:` のまま残し、共有の catalog 定義側のバージョンだけを更新します。`pnpm-workspace.yaml` で定義する pnpm の catalogs はまだマニフェストとして解析しないため、pnpm の catalogs を参照する package.json の `catalog:` は、安全のためスキップします。

TOML マニフェストでは、基本文字列（`"..."`）とリテラル文字列（`'...'`）のどちらも、対応する依存セクション内なら引用符を保ったまま更新します。`Cargo.toml` で書き換えるのは `[dependencies]`、`[dev-dependencies]`、`[build-dependencies]`、`[workspace.dependencies]`、ターゲット固有の依存テーブルだけで、メタデータのテーブルは変更しません。Cargo の git 依存の tag の更新も、inline table と複数行テーブルの両方で同じセクション制限に従い、単一引用符・二重引用符を保持します。`[patch.<registry>]` / `[patch.<registry>.<package>]` も更新対象です。`crates-io` 以外の `registry` を指定した Cargo の依存は、depup が crates.io しか問い合わせないためスキップします。

Cargo の比較演算子による範囲指定は、`>=1.0, <2.0, >=1.0.100` のように 3 個以上の要件をカンマで区切った形式にも対応します。`^1.2.2, <1.5` のようにキャレット・チルダ・ワイルドカードと比較演算子を混ぜた複数要件も、`semver::VersionReq` で有効性を確かめたうえで範囲指定として検出します。上限がなく複数の下限が混在していて安全に書き換えられないものは、スキップとして表示します。

PEP 621 / PEP 735 / Poetry に加えて、uv の旧形式 `[tool.uv] dev-dependencies` と PDM の `[tool.pdm.dev-dependencies]` も読み取ります。`[tool.uv.sources]` で PyPI 以外を指す依存（`workspace = true` / `git` / `path` / `url` / `pypi` 以外の `index`）は、ワークスペースのメンバーやカスタムインデックスの依存を PyPI の同名パッケージで上書きしないよう、スキップします。Poetry の複数行の依存テーブル（`[tool.poetry.dependencies.<name>]` / `[tool.poetry.group.<g>.dependencies.<name>]`）と、TOML の引用符付きキー（`"zope.interface"` / `"ruamel.yaml"`）も解析・更新します。ドットを含む名前は TOML では引用符で囲む必要があるため、パーサーが読む範囲と書き換える範囲を一致させています。

`[project]` / `[tool.rye]` / `[tool.uv]` セクションで書き換えるのは `dependencies` / `dev-dependencies` 配列だけで、`name` / `description` / `keywords` などのメタデータ文字列は、PEP 508 の依存指定に見えても書き換えません。PEP 508 のバージョン指定は `>=3.5,<4,` のような末尾カンマを許しており、depup は下限を更新するときもこのカンマを残します。`pypi` 以外の `source` を指定した Poetry の依存は、PEP 621 の依存を `tool.poetry.dependencies` で補足している場合も含めて、depup が PyPI しか問い合わせないためスキップします。Poetry の複数制約の配列形式（`foo = [{version = "<=1.9", python = ">=3.6,<3.8"}, {version = "^2.0", python = ">=3.8"}]`）も、要素ごとに `requires_python` を判定しない限り配列の要素を安全に書き換えられないため、スキップします。

Gradle の文字列記法では、`:resources@zip` や `@aar` のような classifier / extension のサフィックスを保持します。`//` の行コメントや `/* ... */` のブロックコメントの中だけにある依存宣言は、更新対象にしません。Gradle version catalog では、バージョンが宣言されている TOML の文字列形式またはテーブル形式を保ったまま更新します。

npm の comparator set（空白で区切った比較演算子の組）では、`1.2 <2.0.0` のような演算子のない部分バージョンの下限も扱い、下限側を更新するときは部分バージョンの形を保ちます。

Swift の GitHub 依存では、HTTPS の URL、scp 形式の SSH URL（`git@github.com:owner/repo.git`）、標準の SSH URL（`ssh://git@github.com/owner/repo.git`）、GitHub の SSH over 443 の URL（`ssh://git@ssh.github.com:443/owner/repo.git`）を解析できます。GitHub のタグは `v1.2.3` と `V1.2.3` の両方を認識しますが、`Package.swift` の version requirement 文字列は厳格な SemVer（`X.Y.Z`、先頭ゼロなし）として検証します。また、`Package.swift` の `//` 行コメントや `/* ... */` ブロックコメントの中に書かれた依存宣言は、解析対象から除外します。

SPM の SemVer 2.0.0 準拠に合わせて、プレリリース識別子付きのバージョン（`1.0.0-beta.1`）、ビルドメタデータ付きのバージョン（`1.0.0+build.123`）、両者を組み合わせた形式（`1.0.0-rc.1+sha.abc`）も解析・更新できます。

`.package(...)` の末尾に `traits: [...]`（SPM 6.1 の Package Traits）や `moduleAliases: [...]` のような追加の引数があっても、version requirement だけを置き換えて追加の引数は保持します。Swift Package Registry の `id:` 依存（`.package(id: "scope.name", ...)`）は、registry API のアダプターが未実装のため現在は対応しておらず、スキップします（対象は GitHub URL の依存だけです。将来対応する予定です）。

`go.mod` では、`) // direct deps` のようにコメントが付いたブロックの終端も通常の終端として扱い、`require` / `replace` / `exclude` ブロックの解析と更新に反映します。

`require "golang.org/x/text" "v0.14.0"` のように引用符で囲まれたモジュールパスやバージョンも解析し、引用符を保ったまま更新します。

`go.mod` の単一行・ブロック形式の `require` を更新するときは、元の改行コード（LF / CRLF）を維持します。

## mise のツールバージョン

mise（[jdx/mise](https://mise.jdx.dev)）の設定ファイルに書かれたツールバージョンも、他の言語と同じワークフロー（検出 → 解析 → 判定 → 書き込み）で更新します。バージョン一覧は `mise ls-remote <tool> --json` から取得するため、mise が対応するすべてのバックエンド（core / aqua / ubi / asdf / npm: / cargo: / go: / pipx: など）をそのまま扱えます。

### 対象ファイル

`mise.toml` / `.mise.toml` / `mise/config.toml` / `.mise/config.toml` / `.config/mise.toml` / `.config/mise/config.toml` / `.tool-versions`

`mise.local.toml`（個人のローカル上書き。通常は gitignore の対象）と `mise.<env>.toml`（環境別の上書き設定）は、特定の環境向けの設定だけを知らないうちに書き換えないよう、意図的に対象外にしています。

### バージョン指定の扱い

| 記法 | 例 | 扱い |
|------|-----|------|
| 完全一致 | `node = "26.7.0"` | 最新版へ更新 |
| 前方一致 | `node = "26"` / `"26.7"` | セグメント数を保って更新（`26` → `27`、`26.7` → `26.8`） |
| 明示セレクター | `go = "prefix:1.19"` | `prefix:` を保持して更新（`prefix:1.24`） |
| ベンダー付き | `java = "temurin-21.0.5"` | 同じベンダー内で更新（`temurin-21.0.9`） |
| inline table | `python = { version = "3.13", virtualenv = ".venv" }` | `version` だけ更新し、他のオプションは保持 |
| テーブル形式 | `[tools.terraform]` + `version = "1.15.0"` | 同上 |
| 浮動指定 | `latest` / `lts` / `system` | 意味が変わらないため更新対象外 |
| 非バージョン | `ref:master` / `path:./shfmt` / `sub-2:lts` | 更新対象外 |
| 複数バージョン | `python = ["3.12", "3.13"]` | どれを更新すべきか決められないため対象外 |

`mise ls-remote java` が返す 3000 件超の候補の大半は、`temurin-` / `graalvm-community-` / `zulu-` などのベンダー接頭辞付きです。depup は現在の指定と同じ接頭辞の候補だけを対象にするため、`temurin-21` を使っているプロジェクトが `zulu-27` に書き換わることはありません。

`[tools]` セクションだけを書き換え、`[settings]` / `[env]` / `[tasks]` / `[alias]` に同名のキーがあっても変更しません。引用符の種類（`"` / `'`）、行末コメント、CRLF、`.tool-versions` の空白の並びはすべて保持します。

### mise と age フィルター

mise の `[settings] minimum_release_age` が**明示的に書かれている**場合は、pnpm / bun の `minimumReleaseAge` と同じ「プロジェクトポリシー」として、CLI の `--age` より優先します（mise のデフォルトの 24h は「未設定」とみなし、採用しません）。

```toml
[settings]
minimum_release_age = "7d"   # s / m（分）/ h / d / w / M / y
```

> **注意**: mise の `m` は**分**（humantime 準拠）で、depup CLI の `--age 1m`（1 か月）とは単位が違います。

`mise ls-remote` はデフォルトで mise 側の `minimum_release_age` を適用して新しいバージョンを隠しますが、depup は `--minimum-release-age 0` を渡してすべて取得し、age の判定を depup 側に一本化します。`minimum_release_age_excludes` は depup では解釈しないため、設定されている場合は警告を表示します。

`--install` では `mise install` を実行し、解決済みの age を `MISE_MINIMUM_RELEASE_AGE` として渡します。

## age フィルター

`--age` オプションを使うと、公開から一定期間が経過したバージョンにのみ更新するため、公開直後の不安定なリリースを避けられます。**デフォルトで 1 週間（`1w`）の age フィルターが適用されます**（明示的に上書きしない限り）。

```bash
# デフォルト（--age 1w を指定したのと同じ）
depup

# 公開から 2 週間以上経過したバージョンにのみ更新
depup --age 2w

# 公開から 10 日以上経過したバージョンにのみ更新
depup --age 10d

# 公開から 1 か月以上経過したバージョンにのみ更新
depup --age 1m

# この実行に限り age フィルターを無効化
depup --no-age
```

### グローバル設定

depup は初回実行時に、コメント付きの雛形で `~/.config/depup/config.toml` を自動生成します。値を編集すると、すべてのプロジェクトに共通するデフォルトを変更できます。

```toml
# ~/.config/depup/config.toml

# depup を実行するたびにデフォルトで適用する age フィルター。
# --age と同じ書式（Nd / Nw / Nm）。省略時は組み込みデフォルト（1w）を使用。
age = "1w"

# 更新候補を OSV.dev の脆弱性データベースで照会する（組み込みデフォルト: true）。
# チェックを常に無効にしたい場合は false にする。
osv = true
```

**優先順位（高い順）**
1. `--no-age`（age フィルターを完全に無効化）
2. CLI の `--age <DURATION>`
3. `~/.config/depup/config.toml` の `age` の値
4. 組み込みデフォルト（`1w`）

### 適用される age の優先順位

プロジェクトに書かれた `minimumReleaseAge` は**プロジェクトポリシー**として最優先で扱い、CLI や設定ファイルの age の値を上書きします。全体の優先順位は次のとおりです。

1. **プロジェクトの `minimumReleaseAge`**（最優先。複数のソースに値があれば、より厳しい値）
2. CLI の `--age <DURATION>`
3. `--no-age`（プロジェクトポリシーがない場合のみ有効）
4. `~/.config/depup/config.toml` の `age` の値
5. 組み込みデフォルトの `1w`

プロジェクトポリシーが CLI の指定を上書きした場合、depup は黄色の警告で、実際に使われたソースを表示します。

```
⚠ --age ignored: project's minimumReleaseAge (14 days from pnpm-workspace.yaml) takes precedence
```

プロジェクトポリシーを回避したい場合は、プロジェクトのファイル側で値を削除または変更してください。

### 対応する `minimumReleaseAge` のソース

**pnpm**（次のいずれか。最初に見つかった値を使用）
- `.npmrc`（`minimum-release-age=10d`）
- `pnpm-workspace.yaml`（`minimumReleaseAge: 14400`、分単位）
- `package.json`（`pnpm.settings.minimumReleaseAge`）

**bun**（`bunfig.toml`）
```toml
[install]
minimumReleaseAge = 259200  # 秒単位（この例は 3 日）
```

**mise**（`mise.toml` などの `[settings]`）
```toml
[settings]
minimum_release_age = "7d"  # s / m（分）/ h / d / w / M / y
```

複数のソースに値がある場合は、**より厳しい**（大きい）値を採用します。

### 推移的依存と age フィルター（Rust）

`--install` を指定すると、depup は install によって解決バージョンが変わったクレートだけを対象に公開日を確認し、age ポリシーに違反するものを差し戻します。

```
Auditing transitive Rust dependencies [██████▒▒▒▒] 18/24 (Auditing hyper)
  . — 13 transitive dep(s) rolled back to satisfy --age:
    hyper 1.11.1 → 1.11.0
```

対象を「変わったもの」に限るのは、crates.io へは 1 秒に 1 リクエストしか送れず、ロックファイル全体（数百クレート）を調べると、画面に何も表示されないまま数分止まってしまうからです。install 前に `Cargo.lock` がなかった場合はすべてのエントリが新規扱いになるため、監査全体を 180 秒で打ち切り、残りは未検証として報告します（実行自体は失敗させません）。

### Swift と age フィルター

GitHub Tags API はタグのリリース日時を返しません。そのため Swift パッケージは、`--age` を指定しても age フィルターの対象外として扱います（常に更新対象に含めます）。

## 更新幅の制限（`--max-change`）

`--max-change <LEVEL>` を指定すると、バージョンをどこまで上げてよいかを制限できます。

```bash
# patch のみ許可（1.0.0 → 1.0.5 OK、1.0.0 → 1.1.0 NG）
depup --max-change patch

# patch と minor を許可（1.0.0 → 1.5.3 OK、1.0.0 → 2.0.0 NG）
depup --max-change minor

# デフォルト（major を含むすべての更新を許可）
depup --max-change major
```

更新候補が上限を超える場合、その依存は `max-change=<LEVEL>` という理由でスキップとして表示されます。tag を追う Cargo の git 依存にも同じ上限を適用します。

### グローバル設定

`~/.config/depup/config.toml` で、デフォルトの上限を設定できます。

```toml
# デフォルトで patch と minor まで許可
max_change = "minor"
```

**優先順位（高い順）**
1. CLI の `--max-change <LEVEL>`
2. `~/.config/depup/config.toml` の `max_change` の値
3. 組み込みデフォルト（制限なし）

## 脆弱性チェック（OSV.dev）

depup は更新候補の各バージョンを [OSV.dev](https://osv.dev/) の公開データベースに問い合わせ、既知の脆弱性があるバージョンを更新対象から除外します。**このチェックはデフォルトで有効**なので、フラグを指定する必要はありません。age フィルターと組み合わせると、公開から十分な期間が経ち、既知の脆弱性もないバージョンの中から最新のものを選べます。

```bash
# OSV チェックはデフォルトで実行される
depup

# 明示的に有効化（グローバル設定の `osv = false` を上書き）
depup --osv

# この実行に限り OSV チェックを無効化（グローバル設定とデフォルトを上書き）
depup --no-osv
```

- OSV.dev の API は公開されており、**認証トークンは不要**です。
- Swift パッケージは対象外です（OSV は GitHub リポジトリの URL をキーにパッケージを管理しており、depup が扱う Swift のパッケージ名の形式とは一致しないため）。
- mise のツールは対象外です（バックエンドごとにバージョン体系も名前空間も異なり、単一の OSV ecosystem に対応付けられないため）。
- API エラーが起きても更新は止めません。該当するバージョンは候補に残し、`--verbose` で警告を表示します。

### フォールバック例

更新候補のバージョンに既知の脆弱性が見つかった場合、depup は 1 つ前の安全なバージョンへ自動でフォールバックします。OSV チェックを通過した更新には `✓ OSV` マークが付きます。

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

`falling back` のメッセージは想定どおりの動作を知らせるもので、終了コードには影響しません。

### グローバル設定

チェックはデフォルトで有効です。常に無効にしたい場合は、自動生成された `~/.config/depup/config.toml` で明示的に無効化します。

```toml
# ~/.config/depup/config.toml

# OSV チェックを毎回行わない（組み込みデフォルトは true）
osv = false
```

**優先順位（高い順）**
1. `--no-osv`（チェックを無効化）
2. CLI の `--osv`
3. `~/.config/depup/config.toml` の `osv` の値
4. 組み込みデフォルト（`true`。OSV チェックを実行）

設定ファイルが存在しない場合、または `osv` キーが書かれていない場合は組み込みデフォルトが適用され、チェックが実行されます。

## uv マルウェアチェック（preview）

Python プロジェクトで `--install` が `uv sync` を呼び出すとき、depup は環境変数 `UV_MALWARE_CHECK=1` を常に付けます。これで有効になるのは [uv の preview 版マルウェアチェック機能](https://astral.sh/blog/uv-audit)です（`uv audit` と同時に発表されましたが、`uv audit` コマンドとは別の機能です）。`uv sync` / `uv add` などの sync 操作のたびに、uv はロック済みの依存解決結果を OSV のマルウェア勧告（MAL advisories）と照合し、マルウェアが見つかれば、悪意のあるパッケージが実行される前に sync を中断します。

- 常に有効で、有効化のためのフラグは不要です。
- この機能に対応していない古い uv では環境変数が無視されるだけなので、常に有効にしても既存環境のビルドは壊れません。
- Astral はこの機能を preview と位置づけており、将来挙動が変わる可能性があります。検査は uv 側で実行され、マルウェアが見つかると uv が sync をエラーで終了し、その終了コードがそのまま depup に伝わります。

PHP プロジェクトを `--install` で処理するとき、depup は `composer install` ではなく `composer update` を実行します。`composer install` は既存のロックファイルを再利用するため、depup が直前に `composer.json` へ書いた制約を反映できません。`composer update` で制約を解決し直し、`composer.lock` を更新します。

## 出力

### 進捗表示

<p align="center">
  <img src="docs/images/scanning.png" alt="depup のスキャン中の表示">
</p>

### テキスト出力（デフォルト）

- `🔧` は開発用の依存（devDependencies など）を示します。
- リリース日時は `(yyyy/mm/dd HH:MM)` 形式で表示します。
- 変更の種類は `[major]` / `[minor]` / `[patch]` で示します。

### JSON 出力

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

### diff 出力

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

複数のサブディレクトリを持つモノレポでは、プロジェクトのルートに `.depup` ファイルを作成し、追加で処理するディレクトリを列挙できます。

```
# .depup
gui       # フロントエンドアプリ
api       # バックエンド API
shared    # 共有ライブラリ
```

ルートディレクトリで `depup` を実行すると、ルート自体と列挙したすべてのディレクトリの依存関係をまとめて更新します。バージョン情報はキャッシュされるため、同じパッケージを取得するのは 1 回だけです。

`--install` を指定した場合は、更新されたマニフェストごとに、最も近い対象ディレクトリでパッケージマネージャーの install を実行します。入れ子になったアプリの依存は、リポジトリのルートではなく各アプリのディレクトリで反映されます。

- `#` 以降はコメント（行頭・行末のどちらでも可）
- 空行は無視
- パスは `.depup` ファイルの配置ディレクトリからの相対パス
- 絶対パス、親ディレクトリ（`..`）への移動、`.depup` の配置ディレクトリの外を指すシンボリックリンクは拒否
- 存在しないディレクトリは警告してスキップ
- ルートディレクトリは常にスキャン対象に含まれる

### pnpm ワークスペース

depup は `pnpm-workspace.yaml` を検出し、すべてのワークスペースパッケージを処理します。`packages` 配列はブロック形式（`- 'packages/*'`）とフロー形式（`packages: ['packages/*', 'apps/*']`）の両方に対応し、否定パターン（`!packages/legacy`）も扱えます。

### Cargo ワークスペース

`[workspace] members`（`crates/*` のような glob パターンを含む）を展開し、`[workspace] exclude` に挙げられたメンバーは除外します。

### Go ワークスペース

`go.work` の `use` ディレクティブ（単一行形式と `use ( ... )` ブロック形式の両方）を展開し、各メンバーモジュールの `go.mod` を処理します。展開しないと、ルートに `go.mod` がない構成では、メンバーの依存がすべて古くても「更新なし」と報告してしまいます。

### Gradle マルチプロジェクト

`settings.gradle` / `settings.gradle.kts` の `include ':app', ':core'`（Groovy 形式と Kotlin DSL 形式の両方）を展開し、各サブプロジェクトの `build.gradle` / `build.gradle.kts` と `buildSrc/` を処理します。依存宣言の大半はサブプロジェクト側にあるため、ルートのビルドファイルだけを見ていると取りこぼします。

展開先のパスには `.depup` と同じ安全性の検査を適用します。絶対パス、`..` による親ディレクトリの参照、プロジェクトの外へ解決されるシンボリックリンクは拒否します。

### Tauri プロジェクト

depup は Tauri プロジェクトの `src-tauri/Cargo.toml` を自動検出します。

#### Tauri バージョン同期

Tauri プロジェクトでは、npm の `@tauri-apps/api` と Rust の `tauri` クレートのメジャー・マイナーバージョンが一致している必要があります。depup はこれらのバージョンを自動的に同期し、ビルドエラーを防ぎます。

```
# エラー例（バージョン不一致）
Found version mismatched Tauri packages:
  tauri (v2.10.1) : @tauri-apps/api (v2.9.1)

# depup が自動的にバージョンを同期
@tauri-apps/api: 2.9.1 → 2.10.0
tauri: 2.9.0 → 2.10.1
```

両方のパッケージが同じメジャー・マイナーバージョン（例: `2.10.x`）になるよう、自動で調整されます。

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

コントリビューションを歓迎します。お気軽にプルリクエストをお送りください。

## ライセンス

[MIT](LICENSE)
