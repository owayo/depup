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
- **マニフェスト更新**: マニフェストファイル（`package.json` や `Cargo.toml` など）内のバージョン指定を直接更新
- **範囲指定の維持**: バージョン範囲の形式（`^`、`~`、`>=`）を保ったまま、上限を壊さずに更新
- **固定指定の検出**: 意図的に固定したバージョンはデフォルトでスキップ
- **age フィルター**: 公開から N 日（または N 週）以上経過したバージョンにのみ更新（デフォルトは 1 週間）
- **脆弱性チェック**: 更新先のバージョンを OSV.dev で照会し、既知の脆弱性があるバージョンを避ける（デフォルトで有効）
- **プロジェクトの age 設定**: pnpm・Bun・mise の設定にある最小公開期間を自動適用
- **Bun Catalogs 対応**: `package.json` の Bun `catalog` / `catalogs` 定義を更新
- **モノレポ対応**: `.depup`、Cargo / pnpm / Go のワークスペース、Gradle マルチプロジェクト、入れ子のパッケージごとの install、Tauri プロジェクト
- **公開日時の表示**: 各バージョンの公開日時を表示
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
- **mise**: mise のツールバージョンを更新する場合にのみ必要です（バージョン一覧を `mise ls-remote` から取得するため）。mise がない環境では mise の設定ファイルを読み飛ばし、警告を 1 回表示して他の言語の更新を続けます。

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

# 公開から 2 週間以上経過したバージョンにのみ更新（デフォルトは 1 週間）
depup --age 2w

# diff を表示して更新
depup --diff
```

## 使い方

### 基本構文

```bash
depup [OPTIONS] [PATH]
```

`PATH` には処理するディレクトリを指定します（省略時はカレントディレクトリ）。`--cd` を指定した場合は、先にそのディレクトリへ移動してから `PATH` を解釈します。

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
| `--include-pinned` | | 固定指定も更新対象に含める |
| `--age <DURATION>` | | 公開からの最小経過期間（例: `2w`、`10d`、`1m`）。グローバル設定を上書き |
| `--no-age` | | この実行に限り age フィルターを無効化（グローバル設定とデフォルトを上書き。プロジェクトの `minimumReleaseAge` は上書きしない） |
| `--osv` | | 更新候補を OSV.dev の脆弱性データベースで照会し、既知の脆弱性があるバージョンを回避（デフォルトで有効） |
| `--no-osv` | | この実行に限り OSV の脆弱性チェックを無効化（グローバル設定とデフォルトを上書き） |
| `--max-change <LEVEL>` | | 許容する更新の上限。`patch`（patch のみ）/ `minor`（patch と minor）/ `major`（すべて許可、デフォルト） |
| `--json` | | JSON 形式で出力 |
| `--diff` | | 変更内容を diff 形式で表示 |
| `--install` | | 更新後にパッケージマネージャーの install を実行 |
| `--version` | `-V` | バージョンを表示 |
| `--help` | `-h` | ヘルプを表示 |

`--only` と `--exclude` の両方を指定した場合は、`--only` が優先されます。`--only` で明示したパッケージは、同じ名前を `--exclude` に指定していても更新対象に残ります。

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

## 更新の基本ルール

depup は、固定されていない依存について公開済みのバージョンを調べ、以下の条件を満たす最新のものを選んでマニフェストを書き換えます。デフォルトの条件は次のとおりです。

- `package.json` の `"1.2.3"` のようにバージョンを 1 つに決めた指定は、意図的な固定（固定指定）とみなし、`--include-pinned` を付けない限り更新しません。Go と mise は例外です（[固定指定と `--include-pinned`](#固定指定と---include-pinned)）。
- 公開から 1 週間以上経過したバージョンだけを候補にします（[age フィルター](#age-フィルター)）。
- 既知の脆弱性があるバージョンは避けます（[脆弱性チェック](#脆弱性チェックosvdev)）。
- 現在のバージョンが安定版なら、プレリリースは提案しません（[候補のバージョン順序とプレリリース](#候補のバージョン順序とプレリリース)）。
- 範囲指定は形と上限を保ち、下限だけを進めます（[範囲の上限と下限](#範囲の上限と下限)）。
- メジャーバージョンを上げる更新も許可します。制限するには `--max-change` を使います（[更新幅の制限](#更新幅の制限--max-change)）。

この README では、depup が更新しなかった依存を次の 2 つの用語で書き分けます。

- **スキップ**: 依存として認識したが、更新しなかったもの。出力の件数に含まれ、`--verbose` を付けると依存ごとに理由（`pinned`、`latest` など）を表示します（[更新されないとき](#更新されないとき)）。
- **対象外**: 更新できる依存として扱わず、出力にも現れないもの。レジストリ以外を指す宣言（Cargo の `path` 依存など。Cargo の git 依存は例外で、`git ls-remote` で確認します）、プラットフォームパッケージ（Composer の `php` など）、書き方が浮動指定や未対応のバージョン（`"*"`、`latest` など）がこれにあたります。

age フィルターや脆弱性チェックは、バージョンを候補から外すだけで、依存そのものは外しません。残った候補の中で最新のものへ更新し、現在のバージョンより新しい候補が 1 つも残らなかったときだけスキップとして報告します。

## 更新候補の絞り込み

設定で変えられる 3 つのフィルターが、選べるバージョンを絞り込みます。デフォルトで有効な age フィルターと脆弱性チェック、それに指定したときだけ効く `--max-change` です。どれも、実行ごとにオプションで指定することも、[グローバル設定ファイル](#グローバル設定ファイル)でデフォルトを変えることもできます。プレリリースや、範囲の上限を超えるバージョンも候補から外れます（[バージョン指定と書き換え](#バージョン指定と書き換え)）。

### age フィルター

`--age` オプションを指定すると、公開から一定期間が経過したバージョンにのみ更新します。公開直後の不安定なリリースを避けるための機能です。明示的に上書きしない限り、**デフォルトで 1 週間（`1w`）の age フィルターが適用されます**。

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

age フィルターを適用するのは、depup がマニフェストに書き込むバージョンです。`--install` で解決される推移的依存の扱いは、[推移的依存と age フィルター](#推移的依存と-age-フィルター)を参照してください。Swift パッケージには age フィルターが実質的に効きません。GitHub Tags API はタグの公開日時を返さないため、`--age` の値にかかわらず、すべてのタグが条件を満たします。Cargo の git 依存にも age フィルターは適用しません。

#### 適用される age の優先順位

プロジェクトに書かれた最小公開期間（pnpm・Bun の `minimumReleaseAge`、mise の `minimum_release_age`）は**プロジェクトポリシー**として扱い、CLI の `--age` やグローバル設定ファイルの値より優先します。実際に適用される age は、次の順で決まります（上ほど優先）。

1. プロジェクトポリシー（pnpm・Bun・mise の設定。読み取るファイルと、複数ある場合の扱いは下記）
2. CLI の `--age <DURATION>` または `--no-age`（両者は同時に指定できません。`--no-age` が効くのは、プロジェクトポリシーがない場合だけです）
3. `~/.config/depup/config.toml` の `age`（[グローバル設定ファイル](#グローバル設定ファイル)を参照）
4. 組み込みデフォルトの `1w`

プロジェクトポリシーが CLI の指定を上書きした場合、depup は実際に使われた設定ファイルを黄色の警告で表示します。

```
⚠ --age ignored: project's minimumReleaseAge (14 days from pnpm-workspace.yaml) takes precedence
```

プロジェクトポリシーは `--age` でも `--no-age` でも上書きできません。別の値を使いたい場合は、プロジェクトのファイル側で値を変更または削除してください。

#### `minimumReleaseAge` を読み取る設定ファイル

**pnpm**（`.npmrc` → `pnpm-workspace.yaml` → `package.json` の順に探し、最初に見つかった値を使用）
- `.npmrc`（`minimum-release-age=10d`）
- `pnpm-workspace.yaml`（`minimumReleaseAge: 14400`、分単位）
- `package.json`（`pnpm.settings.minimumReleaseAge`）

**Bun**（`bunfig.toml`）

```toml
[install]
minimumReleaseAge = 259200  # 秒単位（この例は 3 日）
```

**mise**（`mise.toml` などの `[settings]`。`m` が分を表す点に注意。[mise と age フィルター](#mise-と-age-フィルター)を参照）

```toml
[settings]
minimum_release_age = "7d"  # s / m（分）/ h / d / w / M / y
```

pnpm・Bun・mise のうち複数に値がある場合は、より厳しい（大きい）値を採用します。pnpm の中では、最初に見つかった値だけを使います。

### 脆弱性チェック（OSV.dev）

depup は、更新先に選んだバージョンを [OSV.dev](https://osv.dev/) の公開データベースで照会します。既知の脆弱性があればそのバージョンを候補から外し、1 つ古い候補を同じように調べます。**このチェックはデフォルトで有効**なので、フラグを指定する必要はありません。age フィルターと組み合わせると、公開から十分な期間が経ち、既知の脆弱性もないバージョンの中から最新のものが自動で選ばれます。

```bash
# OSV チェックはデフォルトで実行される
depup

# 明示的に有効化（グローバル設定の `osv = false` を上書き）
depup --osv

# この実行に限り OSV チェックを無効化（グローバル設定とデフォルトを上書き）
depup --no-osv
```

- OSV.dev の API は公開されており、認証トークンは不要です。
- Swift パッケージは照会しません。OSV は Swift のパッケージをリポジトリの完全な URL で識別しますが、depup は GitHub の `owner/repo` で識別するため、照会しても一致しません。
- mise のツールも照会しません。バックエンドごとにバージョン体系も名前空間も異なり、OSV の 1 つのエコシステムに対応付けられないためです。
- Cargo の git 依存も照会しません。
- OSV への問い合わせに失敗しても、更新は止めません。脆弱性を確認できていないバージョンがそのまま更新先になり、`✓ OSV` マークは付きません。失敗は `Errors:` 欄（JSON では `errors` 配列）に表示されますが、終了コードは変わりません。

**優先順位（高い順）**
1. CLI の `--osv` または `--no-osv`（両者は同時に指定できません）
2. `~/.config/depup/config.toml` の `osv`
3. 組み込みデフォルト（`true`。チェックを実行）

毎回の実行でチェックを無効にするには、[グローバル設定ファイル](#グローバル設定ファイル)で `osv = false` にします。

#### フォールバック例

更新先に選んだバージョンに既知の脆弱性が見つかると、depup はそれを候補から外して 1 つ古い候補を調べます。安全なバージョンが見つかるか、現在より新しい候補がなくなるまで繰り返します。OSV チェックを通過した更新には `✓ OSV` マークが付きます。

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

この例では、dompurify の 3.4.8 を候補から外したあと、条件を満たす候補が残りませんでした。そのため dompurify は更新されず、41 件のスキップに含まれています。安全なバージョンが見つかった場合は、更新行の下に `↳ OSV skipped: <バージョン> (<勧告 ID>)` 行が付きます。

`falling back` は、脆弱なバージョンを設計どおりに避けたことを知らせる通知で、終了コードには影響しません。レポートの上にある `⚠ OSV:` 行は標準エラー出力への進捗表示です。標準エラー出力が端末のときに表示され、`--quiet` を付けたときは端末かどうかにかかわらず表示されます。デフォルトのテキスト出力では同じ内容が `Errors:` 欄（JSON では `errors`）にも入るため、CI でも失われません。

### 更新幅の制限（`--max-change`）

`--max-change <LEVEL>` を指定すると、バージョンをどこまで上げてよいかを制限できます。

```bash
# patch のみ許可（1.0.0 → 1.0.5 OK、1.0.0 → 1.1.0 NG）
depup --max-change patch

# patch と minor を許可（1.0.0 → 1.5.3 OK、1.0.0 → 2.0.0 NG）
depup --max-change minor

# デフォルト（major を含むすべての更新を許可）
depup --max-change major
```

新しい候補がすべて上限を超える場合は、`max-change=<LEVEL>` という理由でスキップします。上限内に新しい候補があれば、その中の最新へ更新します。タグを追う Cargo の git 依存は扱いが異なり、最新のタグだけを見て、それが上限を超えていれば `max-change=<LEVEL>` としてスキップします。

**優先順位（高い順）**
1. CLI の `--max-change <LEVEL>`
2. `~/.config/depup/config.toml` の `max_change`
3. 組み込みデフォルト（制限なし）

### グローバル設定ファイル

depup は初回実行時に、デフォルトの設定を説明コメント付きで書いた `~/.config/depup/config.toml` を作成します。既存のファイルは上書きしません。値を編集すると、すべてのプロジェクトに共通するデフォルトを変えられます。1 回の実行だけ変えたいときは、コマンドラインのオプションを使います。オプションは設定ファイルの値より優先されます（各節の優先順位を参照）。次の例は生成される雛形の内容です（実際のファイルのコメントは英語です）。

```toml
# depup のグローバル設定
# https://github.com/owayo/depup
#
# このファイルは初回実行時に自動生成されます。
# 下の値を編集すると、depup の組み込みデフォルトを上書きできます。

# depup を実行するたびに適用する age フィルターのデフォルト。
# --age と同じ書式: Nd（日）、Nw（週）、Nm（か月）。
# 実行ごとに --age <DURATION> で上書き、または --no-age で無効化できます。
age = "1w"

# 更新候補を OSV.dev の脆弱性データベースで照会し、
# 既知の脆弱性があるバージョンを候補から外します（デフォルトで有効）。
# ネットワーク接続が必要です。API エラー時は元の候補を採用します。
# 実行ごとに --osv / --no-osv で上書きできます。
osv = true

# 許容するバージョン変更の上限。
# "patch"（patch のみ）、"minor"（patch と minor）、
# "major"（デフォルト。すべて許可）のいずれかを指定します。
# 実行ごとに --max-change <LEVEL> で上書きできます。
# max_change = "minor"
```

ファイルにないキーには、組み込みデフォルト（`age = "1w"`、`osv = true`、`max_change` の制限なし）が使われます。ファイルを作成・解析できない場合は、警告を表示して組み込みデフォルトを使います。値が不正なキー（`age = "abc"` など）は、警告を表示したうえで、そのキーだけを組み込みデフォルトに戻します。

## バージョン指定と書き換え

この章の規則は、すべてのエコシステムに共通します。扱うのは、どの指定を固定とみなすか、範囲をどう進めるか、どの形式を保つか、どの制約を書き換えないか、候補をどの順で比べるか、書き込みで何を保つかです。1 つのエコシステムだけに関わる規則は「[エコシステム別の詳細](#エコシステム別の詳細)」にまとめています。

### 固定指定と `--include-pinned`

バージョンを 1 つに決めた指定（完全一致）は意図的な固定とみなし、デフォルトでは `pinned` としてスキップします。この README では、これを固定指定と呼びます。

| 言語 | 指定例 | デフォルトで更新 |
|------|--------|------------------|
| Node.js | `"1.2.3"` | ❌ |
| Node.js | `"^1.2.3"`、`"~1.2.3"`、`"=1.2"` | ✅ |
| Python | `"==1.2.3"`、Poetry の `"1.2.3"` | ❌ |
| Python | `">=1.2.3"`、`"^1.2.3"` | ✅ |
| Rust | `"=1.2.3"` | ❌ |
| Rust | `"1.2.3"`、`"^1.2.3"` | ✅ |
| Go | `// pinned` コメント付きの `v1.2.3` | ❌ |
| Go | `v1.2.3` | ✅ |
| Ruby | `'1.2.3'`、`'= 1.2.3'` | ❌ |
| Ruby | `'~> 1.2.3'`、`'>= 1.2.3'` | ✅ |
| PHP | `"1.2.3"` | ❌ |
| PHP | `"^1.2.3"`、`"~1.2.3"` | ✅ |
| Java | Gradle の完全一致（`'g:a:1.2.3'`） | ❌ |
| Java | Gradle の strict 記法（`1.2.3!!`） | ❌ |
| Java | Maven の Hard requirement（`[1.0]`） | ❌ |
| Java | Gradle の動的バージョンや範囲（`5.3.+`、`[1.7, 1.8[!!`） | ✅ |
| Swift | `exact: "1.2.3"` | ❌ |
| Swift | `from: "1.2.3"`、`.upToNextMinor` | ✅ |
| mise | `node = "26.7.0"` | ✅ |

`--include-pinned` を付けると、固定指定も更新対象になります。付けない場合は固定指定の依存をレジストリに問い合わせないため、新しいバージョンがあるかどうかも出力からは分かりません。

> **注意**: Go と mise は例外で、完全一致のバージョンを固定指定とみなさず、`--include-pinned` なしで更新します。Go の依存を据え置くには、その行に `// pinned` コメントを付けます（[Go](#go)）。mise のツールを据え置くには `--exclude <ツール名>` を指定します。名前はすべての言語で照合されるため、たとえば `--exclude node` は npm の `node` パッケージも除外します。

### 範囲の上限と下限

depup は上限付きの範囲制約を守ります。排他的な上限（`<` など）と包含的な上限（`<=` など）のどちらにも対応します。

```
">=3.5.0,<4.0.0"   → ">=3.9.1,<4.0.0"
">=1.0,<=2.0"      → ">=2.0,<=2.0"
"4.0.0..<5.0.0"    → "4.99.0..<5.0.0"
"4.0.0...4.9.9"    → "4.9.9...4.9.9"
"1.2.0 - 2.0.0"    → "1.9.3 - 2.0.0" （npm のハイフンレンジ）
"1.0 - 2.0"        → "2.0.9 - 2.0" （npm / Composer の部分バージョンの上限は `<2.1` に展開）
"[1.0,2.0)"        → "[1.9.3,2.0)" （Maven 形式）
"[1.0,2.0]"        → "[2.0,2.0]" （Maven 形式）
"[1.0,2.0.Final)"  → "[1.9.3,2.0.Final)" （Maven の qualifier）
"[1.0,2.0-beta1-SNAPSHOT)" → "[1.9.3,2.0-beta1-SNAPSHOT)" （ハイフンで区切った複数部分からなる Maven の qualifier）
"[1.0,2.0["        → "[1.9.3,2.0[" （Maven の排他的な上限の別表記 `[`）
"<4.0.0"           → スキップ（上限のみの制約）
">1.0.0"           → スキップ（排他的な下限）
"]1.0,2.0["        → スキップ（Maven の排他的な下限）
```

依存に上限付きの範囲（例: `>=3.5.0,<4.0.0`、`>=1.0,<=2.0`、`4.0.0...4.9.9`）がある場合、depup は次のように動作します。

- 上限を超えるバージョンは提案しません。
- 包含的な上限（`<=`、`...`）では、上限値そのものも候補に含めます。
- マニフェストファイル内の元の制約の形を維持します。
- 範囲内の最新バージョンに合わせて、下限側だけを更新します。

npm / Composer のハイフンレンジで、`1.0 - 2.0` のように右辺が部分バージョンのときは、右辺の `2.0` を `2.0.x` とみなし、排他的な上限 `<2.1` として解釈します。そのため `2.0.x` は候補に残り、`2.1.0` 以降は候補から外れます。

### 元の指定形式の維持

depup は元のバージョン範囲の形式を維持します。チルダ（`~`）のように、書かれたセグメント（`.` で区切った数字）の個数で許容幅が決まる指定では、その個数も保ちます。最後のグループは固定指定なので、`--include-pinned` を付けたときだけ更新します。

```
# 演算子とチルダの幅（npm / Cargo / Composer / RubyGems）
"^1.2.3" → "^2.0.0"  （キャレット維持）
"~1.2.3" → "~1.3.0"  （チルダ維持）
"~1.2"   → "~1.9"    （チルダのセグメント数を維持。`~1.9.3` のように増やすと許容幅が狭まる）
"~1"     → "~2"      （1 セグメントのチルダはメジャー単位の幅を維持）
"~1.2 <2.0.0" → "~1.9 <2.0.0" （空白区切りで比較演算子と組み合わせたチルダもセグメント数を維持）
"~1, <5.0" → "~4, <5.0" （Cargo の複数要件でもチルダの幅を維持）
"~> 7.0" → "~> 8.1"  （RubyGems の悲観的演算子、セグメント数を維持）
">=1.0.0" → ">=2.0.0" （範囲維持）

# Python（pyproject.toml）
"requests (>=2.28,<3); python_version < '3.12'" → "requests (>=2.31,<3); python_version < '3.12'" （PEP 508 の括弧とマーカーを維持）
"coverage [toml] >=7,<8" → "coverage [toml] >=7.6,<8" （PEP 508 extras の空白を維持）
"'paramiko>=3.5.0,<4.0.0,'" → "'paramiko>=3.9.1,<4.0.0,'" （PEP 508 の末尾カンマを維持）
"'paramiko>=3.5.0,<4.0.0'" → "'paramiko>=3.9.1,<4.0.0'" （TOML リテラル文字列の引用符を維持）

# ワイルドカード・x-range・部分バージョン
"1.x" → "2.x" （ワイルドカード形式を維持）
"1.2.x - 2.3.x" → "1.9.x - 2.3.x" （npm のハイフンレンジ、端点がワイルドカード）
"1.x.x" → "2.x.x" （複数のワイルドカード位置を維持）
"1.2.*" → "1.3.*" （ワイルドカード形式を維持）
"v1.*" → "v2.*" （先頭の `v` を維持）
"V1.*" → "V2.*" （Composer の大文字 `V` を維持）
"^1.x" → "^2.x" （npm のキャレット + ワイルドカード、演算子を維持）
"~1.2.x" → "~2.3.x" （npm のチルダ + ワイルドカード、演算子を維持）
"=1.x" → "=2.x" （npm の等号 + ワイルドカード、演算子を維持）
"=1.2" → "=2.3" （npm の partial comparator、演算子を維持）

# Gradle / Maven
"5.3.+" → "5.4.+" （Gradle のプレフィックスを維持）
"5.3.+!!" → "6.1.+!!" （Gradle strict の動的プレフィックスを維持）
"[1.7, 1.8[!!" → "[1.7.36, 1.8[!!" （prefer なしの Gradle strict 範囲）
prefer("1.7.25") → prefer("1.7.36") （Gradle rich version の strict 範囲内の prefer）
"org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.25" → "org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.36" （Gradle strict 範囲の prefer 短縮記法）

# Gradle / Maven の固定指定（--include-pinned を付けたときだけ更新）
"1.2.3!!" → "2.0.0!!" （Gradle strict を維持）
"[1.0]" → "[2.0]" （Maven の Hard requirement を維持）
"[1.2.3.Final]" → "[1.3.0]" （Maven の Hard requirement、qualifier 付き）
group = "com.google.guava", name = "guava", version = "32.1.2-jre" → version = "33.4.0-jre" （Gradle Kotlin の map 記法）
junit = "junit:junit:4.13.2" → "junit:junit:4.13.3" （Gradle version catalog の library）
guava = "32.1.2-jre" → "33.4.0-jre" （Gradle version catalog の version 参照）
"group:name:1.0.0:classifier@zip" → "group:name:1.1.0:classifier@zip" （Gradle の classifier / extension を維持）
```

### 自動更新しない制約

安全に書き換えられない制約は、部分的に更新せずにスキップします。新しいバージョンがある場合の理由は `parse error: constraint cannot be updated safely` で、ない場合は `latest` です。主な例は次のとおりです。

- npm / Composer の OR 制約（`^1 || ^2`）と、Composer の後方互換表記である単一パイプ（`^1 | ^2`）
- `!=` を含む除外制約（`!=1.2.3`、`>=1.0, !=1.5.0, <2.0`）と、Composer で「等しくない」を `<>` と書いた除外制約（`>=1.0 <>1.5.0 <2.0`、`>=1.0,<>1.5.0,<2.0`）
- 上限のみの制約（`<4.0.0`、`<=2.0`）
- 排他的な下限の制約（`>1.0.0`）
- 下限のない Maven 形式の範囲（`(,2.0]`）と、下限が排他的な Maven 形式の範囲（`]1.0,2.0[`）

npm には `!=` という比較演算子がないため、npm の制約に `!=` が含まれていれば対象外にします。

次の指定はスキップではなく対象外にし、出力にも出しません。

- 完全な浮動指定（常に最新版を指す指定）。`"*"`、npm の dist-tag（`"latest"` など）、Gradle の `"latest.release"` / `"latest.integration"` / `"latest.milestone"` / ユーザー定義の `latest.<status>` が該当します。書き換えると、完全一致のバージョンに変わってしまうためです。
- 数字を含まない複数セグメントのワイルドカード（Composer の `*.*` / `v*` / `V*` / `x.x`）と、下限も上限も空の Maven 形式の範囲（Java / Gradle の `[,]` / `(,)`）。受け付けると、何も変わらない更新を毎回報告したり、「常に古い」と誤判定したりするためです。
- `1.x.3` や `^x.0.0` のように、ワイルドカード文字（`x` / `X` / `*`）の後ろに数値セグメントが続く形式。node-semver や Rust の semver クレートでは無効な x-range で、受け付けると不正な制約を書き出すため、解析の時点で対象外にします。

### 候補のバージョン順序とプレリリース

更新候補のバージョンは、エコシステムごとの規則で大小を比べます。

| エコシステム | 比較の規則 |
|--------------|------------|
| Node.js / Rust / Go / Swift | SemVer。`1.0.0-1` のように `-` の後ろが数字だけの場合もプレリリースとみなし、`1.0.0` より小さいものとして扱う。ビルドメタデータは大小の比較で無視するため、`1.1.3` と `1.1.3+spec-1.1.0` の違いだけでは更新しない |
| Python | PEP 440 の正規化と順序 |
| Ruby | RubyGems のセグメント順。英字またはハイフンを含むバージョンはプレリリース |
| PHP | composer/semver の規則。patch alias（`-p1` / `-pl1` / `-patch1`）は、対応する安定版より新しいものとして扱う |
| Java / mise | Gradle 公式のバージョン順序 |

数値セグメントは桁数の上限なしで比べるため、巨大な数値でも桁あふれしません。

現在のバージョンが安定版の間は、プレリリース（alpha・beta・rc・canary・dev など）を候補から外します。現在のバージョンがすでにプレリリースなら、次のプレリリースや安定版へ進めるよう、プレリリースも候補に残します。安定版の依存にプレリリースを提案させるオプションはありません。非推奨を示す接尾辞の付いたバージョンも同じ扱いで、`serde_yaml 0.9.33` を `0.9.34-deprecated` へは更新しません。

### 書き込みの共通原則

depup が書き換えるのは、解析した依存宣言だけです。依存宣言以外の場所は、パッケージ名やバージョンが書かれていても変更しません。たとえば `package.json` の `overrides`、`composer.json` の `replace` / `provide` / `conflict`、`Cargo.toml` や `pyproject.toml` のメタデータのテーブルがこれにあたります（対象のセクションは[エコシステム別の詳細](#エコシステム別の詳細)を参照）。値を書き換えるときは、周囲の書き方も保ちます。TOML マニフェストでは、基本文字列（`"..."`）とリテラル文字列（`'...'`）のどちらも、引用符の種類を変えません。

書き込み先が曖昧なとき、depup は書き込みを拒否し、エラーとして報告します（終了コードは 2）。同じ依存キーがマニフェスト内で複数回宣言されている場合と、複数の Gradle 依存が 1 つのバージョン変数や version catalog の `version.ref` を共有している場合がこれにあたります。固定指定の宣言のように変えてはいけない宣言を、巻き添えで書き換えないためです。

## パッケージマネージャーの実行（`--install`）

`--install` を指定すると、depup はマニフェストを書き換えたあと、プロジェクトごとにパッケージマネージャーを実行します。ロックファイルやインストール済みのパッケージを、新しいバージョンに合わせるためです。

- install を実行するのは、1 件以上更新したマニフェストだけです。`--dry-run` のときは実行しません。
- [`.depup`](#depup-設定ファイル) がない場合は、更新したマニフェストがワークスペースのメンバーのものでも、install はすべて対象ディレクトリ（`PATH` 引数、省略時はカレントディレクトリ）で実行します。`.depup` がある場合は、更新したマニフェストを含む対象ディレクトリのうち最も深いもので実行するため、入れ子のアプリは各自のディレクトリで install されます。
- install は 1 つずつ、ディレクトリのパス順に実行します。同じディレクトリでは、言語ごとに 1 回だけです。パッケージマネージャーの出力は画面に流さずに depup が受け取り、標準エラー出力は install が失敗したときだけ表示します。

install が失敗しても、残りの install は続けます。失敗したコマンドとパッケージマネージャーの標準エラー出力を表示し、最後に `Error: Some package manager installs failed` を出して終了コード 1 で終わります。パッケージマネージャーがインストールされていない場合も失敗として扱います。書き換え済みのマニフェストは元に戻しません。また、どのプロジェクトでも Rust の監査（[推移的依存と age フィルター](#推移的依存と-age-フィルター)）を実行しません。

### パッケージマネージャーごとのコマンド

パッケージマネージャーは、install を実行するディレクトリにあるファイルから判定します。親ディレクトリはたどりません。各言語で、上の行から順に最初に見つかったものを使います。

| 言語 | 判定に使うファイル | 実行するコマンド |
|------|--------------------|------------------|
| Node.js | `pnpm-lock.yaml` | `pnpm install` |
| | `yarn.lock` | `yarn install` |
| | `bun.lock` / `bun.lockb` | `bun install` |
| | `package-lock.json`、または `package.json` のみ | `npm install` |
| Python | `uv.lock` | `uv sync` |
| | `poetry.lock` | `poetry install` |
| | `requirements.lock` / `requirements-dev.lock` | `rye sync` |
| | `Pipfile.lock` | `pipenv install` |
| | `pyproject.toml` / `requirements.txt` | `pip install -e .` |
| Rust | `Cargo.toml`、または Tauri プロジェクトの `src-tauri/Cargo.toml`（このときは `src-tauri/` で実行） | `cargo update` |
| Go | `go.mod` | `go mod download` |
| Ruby | `Gemfile` | `bundle install` |
| PHP | `composer.json` | `composer update` |
| Java | `gradlew` | `./gradlew dependencies` |
| | `build.gradle` / `build.gradle.kts` | `gradle dependencies` |
| Swift | `Package.swift` | `swift package resolve` |
| mise | [mise の設定ファイル](#対象ファイル)のいずれか | `mise install` |

表のファイルがどれもなければ、その言語の install は実行せず、何も表示しません。age フィルターが有効なときは、pnpm・uv・mise に age の値も渡します（[推移的依存と age フィルター](#推移的依存と-age-フィルター)）。`uv sync` には常に `UV_MALWARE_CHECK=1` を付けます（[uv のマルウェアチェック](#uv-のマルウェアチェックpreview)）。

PHP プロジェクトを `--install` で処理するとき、depup は `composer install` ではなく `composer update` を実行します。`composer install` は既存のロックファイルを再利用するため、depup が直前に `composer.json` へ書いた制約を反映できません。`composer update` で制約を解決し直し、`composer.lock` を更新します。

### 推移的依存と age フィルター

age フィルターは、depup がマニフェストに書き込むバージョンを決めるものです。`--install` で解決される推移的依存にも効くかどうかは、パッケージマネージャーによって違います。渡す age の値は、更新の判定に使ったものと同じです（[適用される age の優先順位](#適用される-age-の優先順位)）。

| パッケージマネージャー | depup が渡すもの | 推移的依存への効果 |
|------------------------|------------------|--------------------|
| pnpm | 環境変数 `npm_config_minimum_release_age=<分>` | pnpm v10.16 以降が適用する（それより古い pnpm は環境変数を無視する） |
| uv | `--exclude-newer <日時>` | uv が依存の解決時に適用する |
| Cargo | なし（`cargo update` のあとに depup が `Cargo.lock` を監査する） | depup が条件を満たさないものを差し戻す（下記） |
| mise | 環境変数 `MISE_MINIMUM_RELEASE_AGE=<秒>s` | mise のツールに推移的依存はない。前方一致の指定（`node = "26"` など）を `mise install` が解決するときに適用する |
| npm、Yarn、Bun、pip、Poetry、Rye、Pipenv、Go、Bundler、Composer、Gradle、SwiftPM | なし | 適用されない（age フィルターが効くのは直接依存だけ） |

`--verbose` を付けると、今回使うパッケージマネージャーのうち、age フィルターが直接依存にしか効かないものを通知します。`--no-age` を指定し、プロジェクトポリシーもない場合は、age の値をどこにも渡さず、Rust の監査も行いません。

Rust では、install の前後で `Cargo.lock` のバージョンが変わったクレートだけ公開日時を確認します。age フィルターの条件を満たさないものは、条件を満たす最新のバージョンへ差し戻します。

```
⠙ Auditing hyper [██████████████████████▓░░░░░░░] 18/24 (6s)
  . — 1 transitive dep(s) rolled back to satisfy --age:
    hyper 1.11.1 → 1.11.0
```

対象を「変わったもの」に限るのは、crates.io の利用ポリシーに従ってリクエストを 1 秒に 1 回までに抑えているためです。ロックファイル全体（多くは数百クレート）を調べると、それだけで数分かかります。監査には `Cargo.lock` ごとに 180 秒の上限があり、超えた分は未検証として報告します。install 前に `Cargo.lock` がなかった場合はすべてのエントリが新規扱いになるため、この上限に達しやすくなります。差し戻した結果は常に表示し、差し戻せなかったクレートは `--verbose` のときに表示します。監査の結果で終了コードが変わることはありません。

### uv のマルウェアチェック（preview）

Python プロジェクトで `--install` が `uv sync` を呼び出すとき、depup は環境変数 `UV_MALWARE_CHECK=1` を常に付けます。これで有効になるのは [uv の preview 版マルウェアチェック機能](https://astral.sh/blog/uv-audit)です（`uv audit` と同時に発表されましたが、`uv audit` コマンドとは別の機能です）。`uv sync` / `uv add` などの sync 操作のたびに、uv はロック済みの依存解決結果を OSV のマルウェア勧告（MAL advisories）と照合し、マルウェアが見つかれば、悪意のあるパッケージが実行される前に sync を中断します。

- 常に有効で、有効化のためのフラグは不要です。
- この機能に対応していない古い uv では環境変数が無視されるだけなので、常に有効にしても既存環境のビルドは壊れません。
- Astral はこの機能を preview と位置づけており、将来挙動が変わる可能性があります。
- 検査は uv 側で実行されます。マルウェアが見つかると uv が sync をエラーで終了し、depup も install の失敗として終了コード 1 で終わります。

## 出力と終了コード

### 進捗表示

<p align="center">
  <img src="docs/images/scanning.png" alt="depup のスキャン中の表示">
</p>

### テキスト出力（デフォルト）

- `🔧` は開発依存（devDependencies など）を示します。
- 公開日時は `(yyyy/mm/dd HH:MM)` 形式で表示します。
- 変更の種類は `[major]` / `[minor]` / `[patch]` で示します。
- `✓ OSV` は、脆弱性チェックを通過したバージョンに付きます。更新行の下の `↳ OSV skipped:` 行には、脆弱性があるため候補から外したバージョンを表示します。ラベルは skipped ですが、依存のスキップではありません（依存そのものは更新されています）。
- スキップした依存の件数は、マニフェストの見出し（`— N updates, M skips`）と Summary に表示します。更新が 0 件のマニフェストでは、理由ごとの件数も表示します。
- `--verbose` を付けると、スキップした依存を理由ごとに一覧し、Summary にも理由別・言語別の内訳を付けます。
- エラーは、OSV の通知も含めて `Errors:` 欄に表示します。

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

### 終了コード

| コード | 意味 |
|--------|------|
| `0` | 失敗なし。更新がなかった場合、ドライラン、`Errors:` 欄の内容が OSV の通知（フォールバックや問い合わせの失敗）だけの場合も含む |
| `1` | パッケージマネージャーの install が失敗した、`--cd` でディレクトリを移動できなかった、または depup 自体が動作を続けられなかった（HTTP クライアントの初期化や、結果の出力に失敗した場合など） |
| `2` | 処理の一部が失敗した。マニフェストの読み込み・解析・書き込みの失敗（書き込み先が曖昧なための拒否を含む）と、レジストリからの取得の失敗が該当する。コマンドライン引数の誤りも `2` になる |

終了コード 2 になる失敗が起きても、処理は止まりません。取得に失敗した依存はスキップし、読み込みや解析に失敗したマニフェストは処理から外して、それ以外の書き込みや install は続けます。書き込みに失敗したファイルは元のまま残りますが、その依存は更新として表示され、`--install` も実行されます。`1` と `2` の両方に当てはまる場合は `1` になります。

取得の問題のうち、次の 3 つは終了コードを変えません。

- Cargo の git 依存で `git ls-remote` が失敗した場合（その依存はスキップします）
- 使えるバージョンをレジストリが 1 つも返さない場合（`fetch failed: no versions available`）
- `mise` コマンドがない場合（mise の設定ファイルを警告付きで読み飛ばします）

「更新がある」ことを示す専用の終了コードはないため、CI で結果を調べるには `--json` を使ってください。

エラーは、テキスト出力では `Errors:` 欄に、JSON 出力では `errors` 配列に入ります。`--diff` のとき、またはテキスト出力で `--quiet` を付けたときは、`--verbose` も付けない限りエラーの一覧を表示しません（付けた場合は標準エラー出力に出ます）。失敗の有無は終了コードで確かめてください。

## 更新されないとき

テキスト出力のデフォルトでは、スキップした依存は件数しか表示しません。`--verbose` を付けると、理由ごとに依存を一覧できます。

| 理由 | 意味 | 参照先 |
|------|------|--------|
| `latest` | フィルターを通った候補の中に、現在より新しいバージョンがなかった。新しいリリースがあっても、公開から日が浅い、脆弱性がある、プレリリースである、範囲の上限を超える、のどれかに当たれば `latest` になる | [更新候補の絞り込み](#更新候補の絞り込み)、[範囲の上限と下限](#範囲の上限と下限) |
| `pinned` | 固定指定なので、レジストリに問い合わせていない | [固定指定と `--include-pinned`](#固定指定と---include-pinned) |
| `max-change=<LEVEL>` | 新しいバージョンはあるが、すべて `--max-change` の上限を超えている | [更新幅の制限](#更新幅の制限--max-change) |
| `excluded` / `not in --only` | `--exclude` で除外した、または `--only` に含まれていない | [オプション](#オプション) |
| `no suitable version` | 現在のバージョンも含めて、フィルターを通るバージョンが 1 つもない（例: すべてのリリースが age の基準より新しい） | [更新候補の絞り込み](#更新候補の絞り込み) |
| `parse error: ...` | 制約は読み取れたが、安全に書き換えられない（例: `parse error: constraint cannot be updated safely`）。ラベルに反してマニフェスト自体は正しく解析できており、終了コードも変わらない | [自動更新しない制約](#自動更新しない制約) |
| `fetch failed: ...` | バージョンの取得に失敗した（レジストリのエラーや `git ls-remote` の失敗など）。終了コードが変わるかどうかは原因による | [終了コード](#終了コード) |

表の理由はテキスト出力の表記です。`--verbose` を付けた `--json` の出力では、同じ理由が `already_latest`、`pinned`、`change_level_limited: <LEVEL>`、`excluded`、`not_in_only_list`、`no_suitable_version`、`parse_error: ...`、`fetch_failed: ...` と表記されます。

依存が出力にまったく現れない場合、depup はその宣言を対象外にしています。ファイル・セクション・宣言の書き方が対応しているかを、[エコシステム別の詳細](#エコシステム別の詳細)で確認してください。`--node` のように言語を指定した場合、指定していない言語のマニフェストは解析しません（pnpm・Bun・mise の age 設定だけは読み取ります）。マニフェストは更新されたのに install が失敗した場合は、[パッケージマネージャーの実行](#パッケージマネージャーの実行--install)を参照してください。

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

`--install` を指定した場合は、更新したマニフェストを含む対象ディレクトリのうち最も深いもので、パッケージマネージャーの install を実行します。入れ子になったアプリの install は、リポジトリのルートではなく各アプリのディレクトリで行われます（[パッケージマネージャーの実行](#パッケージマネージャーの実行--install)）。

`.depup` の書式は次のとおりです。

- `#` 以降はコメント（行頭・行末のどちらでも可）
- 空行は無視
- パスは `.depup` ファイルの配置ディレクトリからの相対パス
- 絶対パス、`..` を含むパス、`.depup` の配置ディレクトリの外を指すシンボリックリンクが 1 行でもあると、警告を出して `.depup` 全体を無視（`.depup` がない場合と同じく、ルートと自動検出されるワークスペースを処理）
- 存在しないディレクトリは警告して無視

### pnpm ワークスペース

depup は `pnpm-workspace.yaml` を検出し、すべてのワークスペースパッケージを処理します。`packages` 配列はブロック形式（`- 'packages/*'`）とフロー形式（`packages: ['packages/*', 'apps/*']`）の両方に対応し、否定パターン（`!packages/legacy`）も扱えます。

### Cargo ワークスペース

`[workspace] members`（`crates/*` のような glob パターンを含む）を展開し、`[workspace] exclude` に挙げられたメンバーは処理しません。

### Go ワークスペース

`go.work` の `use` ディレクティブ（単一行形式と `use ( ... )` ブロック形式の両方）を展開し、各メンバーモジュールの `go.mod` を処理します。展開しないと、ルートに `go.mod` がない構成では、各メンバーに古い依存があっても「更新なし」と報告してしまいます。

### Gradle マルチプロジェクト

`settings.gradle` / `settings.gradle.kts` の `include ':app', ':core'`（Groovy 形式と Kotlin DSL 形式の両方）を展開し、各サブプロジェクトの `build.gradle` / `build.gradle.kts` と `buildSrc/` を処理します。依存宣言の大半はサブプロジェクト側にあるため、ルートのビルドファイルだけを見ていると取りこぼします。

`go.work` や `settings.gradle` から展開したパスには、`.depup` と同じ安全性の検査を適用します。絶対パス、`..` による親ディレクトリの参照、プロジェクトの外へ解決されるシンボリックリンクは拒否します。

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

## エコシステム別の詳細

この章は参照用です。depup がどの宣言を読み、どう書き換えるかを正確に知りたいときに、使っているエコシステムの節を引いてください。すべてのエコシステムに共通する規則は「[バージョン指定と書き換え](#バージョン指定と書き換え)」にあります。

### Node.js

`package.json` で更新するのは `dependencies`、`devDependencies`、`peerDependencies`、`optionalDependencies` です。`overrides` などのセクションは変更しません。

チルダの従来形式 `~>1.2.3`（node-semver 互換）も受け付け、更新後も `~>` を保持します。

npm の `=1.2` や `=1` のように、セグメントの一部だけを書いた比較（partial comparator）は固定指定ではありません。node-semver では `=1.2` が「1.2.x のどれでもよい」（`>=1.2.0 <1.3.0` と同じ）という意味になるためです。depup は `=` 演算子と、書かれているセグメント数を保って更新します（`=1.2` → `=2.3`、`=1` → `=2`）。

npm の comparator set（空白で区切った比較演算子の組）では、`1.2 <2.0.0` の `1.2` のように演算子を付けずに書いた部分バージョンも下限として扱い、その形を保ったまま更新します（`1.2 <2.0.0` → `1.9 <2.0.0`）。

node-semver の文法ではハイフンレンジの両端にワイルドカードも書けるため、`1.x - 2.x` のような形も解析・更新します。ただし depup が他の箇所で受け付けない端点（`1.x.3` のようにワイルドカードの後ろに数値が続く形式や、完全な浮動指定の `*`）は、ここでも受け付けません。

npm では、更新対象として解析する前に、バージョン文字列のプレリリース識別子とビルドメタデータ識別子を検証します。アンダースコアを含む識別子（`1.2.3-rc_1`）、空の識別子を含む形式（`1.2.3-alpha..1`）、先頭がゼロの数値プレリリース識別子（`1.2.3-01`）は対象外にします。書き換えると、`package.json` に不正な制約を書き出してしまうためです。

先頭ゼロの検証は、ビルドメタデータを切り落としてから行います。SemVer ではビルド識別子にハイフンや先頭ゼロを含められるため、`1.0.0+2024-01` や `1.2.3+00` のようなバージョンは有効なものとして更新対象に残します（検証するのはプレリリース部分だけです）。

Bun のワークスペースでは、ルートの `package.json` にあるトップレベルの `catalog` / `catalogs` と、`workspaces.catalog` / `workspaces.catalogs` を解析・更新します。ワークスペース内パッケージの `"react": "catalog:"` や `"jest": "catalog:testing"` のような参照は `catalog:` のまま残し、共有の catalog 定義側のバージョンだけを更新します。`pnpm-workspace.yaml` で定義する pnpm の catalogs は、まだマニフェストとして解析していません。pnpm の catalogs を参照する `package.json` の `catalog:` は、安全のため対象外にします。

### Python

PEP 621 / PEP 735 / Poetry に加えて、uv の旧形式 `[tool.uv] dev-dependencies` と PDM の `[tool.pdm.dev-dependencies]` も読み取ります。`[tool.uv.sources]` で PyPI 以外を指す依存（`workspace = true` / `git` / `path` / `url` / `pypi` 以外の `index`）は、ワークスペースのメンバーやカスタムインデックスの依存を PyPI の同名パッケージで上書きしないよう、対象外にします。Poetry の複数行の依存テーブル（`[tool.poetry.dependencies.<name>]` / `[tool.poetry.group.<g>.dependencies.<name>]`）と、TOML の引用符付きキー（`"zope.interface"` / `"ruamel.yaml"`。ドットを含む名前は、TOML では引用符で囲む必要があります）も解析・更新します。

`[project]` / `[tool.rye]` / `[tool.uv]` セクションで書き換えるのは `dependencies` / `dev-dependencies` 配列だけで、`name` / `description` / `keywords` などのメタデータ文字列は、PEP 508 の依存指定に見えても書き換えません。PEP 508 のバージョン指定は `>=3.5,<4,` のような末尾カンマを許しており、depup は下限を更新するときもこのカンマを残します。Poetry で `pypi` 以外の `source` を指定した依存も、depup が PyPI しか問い合わせないため対象外です。PEP 621 の依存を `tool.poetry.dependencies` で補足している場合も同じです。Poetry の複数制約の配列形式（`foo = [{version = "<=1.9", python = ">=3.6,<3.8"}, {version = "^2.0", python = ">=3.8"}]`）も対象外です。depup は配列の要素ごとに `requires_python` を判定しないため、どの要素を書き換えればよいかを安全に決められません。

PyPI 以外をデフォルトのインデックスにしている `pyproject.toml` では、すべての依存を対象外にし、警告を 1 回表示します。該当するのは、Poetry の `priority = "primary"` / `"default"` の source、uv の `[[tool.uv.index]] default = true` や `[tool.uv] index-url`、PDM による `pypi` の上書きです。depup は PyPI しか問い合わせないため、更新すると非公開パッケージが同名の公開パッケージに置き換わります。

Python の互換リリース指定（compatible release、`~=`）は PEP 440 に従います。`~=1.2`（= `>=1.2,<2.0`）と `~=1.2.3`（= `>=1.2.3,<1.3.0`）は上限のある範囲として扱い、その範囲内で更新します。`~=1.2.3` は 1.2 系、`~=1.2` は 1.x 系の中に留まります。書き換えでは元のセグメント数を維持します（`~=1.2` → `~=1.9`。`~=1.9.0` にすると上限が `<1.10.0` に狭まるため）。単一セグメントの `~=1` は無効な形式なので対象外です。

PEP 440 の前方一致（prefix matching）は、`==1.2.*` / `!=1.2.*` のように、リリース番号の末尾に `.*` を付けて `==` か `!=` と組み合わせた指定でだけ受け付けます。`>=1.0.*`、`~=1.0.*`、`==1.0a1.*`、`==1.0.post1.*`、`==1.0+local.*` のような無効な形式は、解析の時点で対象外にします。`===1.0.*` は PEP 440 の arbitrary equality（バージョンとして解釈せず、文字列として完全に一致させる指定）なので、前方一致ではなく固定指定として扱います。

Poetry の `[tool.poetry.dependencies]` では、演算子のないバージョン文字列（`requests = "2.28.0"`）は `==2.28.0` と同じ完全一致の指定です（Poetry 公式ドキュメントの "Exact requirements"）。depup は文字列形式でも inline table（`{ version = "1.26.0", optional = true }`）でも、これを固定指定として `pinned` でスキップします。`--include-pinned` を付けたときは、演算子を付けないまま新しいバージョンへ書き換えます（`4.2.1` → `5.0.0`）。演算子なしを完全一致とみなすのは Poetry の設定内だけです。pip や PEP 508 の依存指定では演算子が必須なので、演算子のないバージョンは受け付けません。

PEP 440 の local version（`+` 以降のラベル）は、SemVer のビルドメタデータとは別物として、Python 固有の意味で扱います。固定指定と除外指定は、local ラベルを保ったまま解析しますが、書き換えません（`torch==2.1.0+cu121`、`!=1.0+local1`）。新しいバージョンがあれば、`parse error: constraint cannot be updated safely` としてスキップします。local ラベルは、同じ公開バージョン（public version）から作られた別ビルド（`cu121` は CUDA 12.1 向け）を指します。公開バージョンだけを進めると、存在しないビルドを指してしまいます。候補の比較では、local version を同じ公開バージョンより新しいものとして扱い（`1.0+local > 1.0`）、local 部分どうしも PEP 440 に従って比較します（`1.0+1 > 1.0+abc`、`1.0+abc.2 > 1.0+abc.1`）。PEP 440 が local ラベルを許していない順序比較・互換リリースの指定（`>=1.0+local`、`~=1.0+local`、`>=1.0+local,<2.0`）は対象外です。

PEP 440 のプレリリースは、区切り文字なしで書かれた場合（例: `2.0.0rc1`、`1.0rc1`、`1.0.0a1`）でも検出し、デフォルトで候補から外します。安定版を使っている依存が、rc 版へ誤って更新されることはありません。ポストリリース（`1.0.post1`）は対応する安定版より新しいものとして比較し、エポック（`1!2.3`）は比較で最優先します。プレリリースに付いたポストリリース（`1.0a1.post1`）も元のプレリリースより新しいものとして扱う（`1.0a1 < 1.0a1.post1 < 1.0`）ため、アルファ版を追っているユーザーもポストリリースの修正を取りこぼしません。

### Rust

`Cargo.toml` で書き換えるのは、`[dependencies]`、`[dev-dependencies]`、`[build-dependencies]`、`[workspace.dependencies]`、ターゲット固有の依存テーブルだけで、メタデータのテーブルは変更しません。

`alias = { package = "actual-crate", version = "1" }` のような Cargo のリネーム依存は、実際のパッケージ名でバージョンを取得し、マニフェスト上のキーへ書き戻します。`--only` / `--exclude` はどちらの名前でも指定できます。

path 依存（`{ path = "../common" }`）は、公開用に `version` を併記していても対象外です。ローカルのクレートで解決されるためです。crates.io 以外のレジストリを指す依存（`crates-io` 以外の `registry = "..."`、または `registry-index = "..."`）も、depup が crates.io しか問い合わせないため対象外です。

git 依存は、レジストリの代わりに `git ls-remote` で確認します。

| 指定 | 動作 |
|------|------|
| `tag` | 最新の安定版の SemVer タグへ書き換える。そのタグが `--max-change` の上限を超える場合は `max-change=<LEVEL>` としてスキップし、上限内の古いタグは候補にしない |
| `branch`、または指定なし（デフォルトブランチ） | リモートの先頭のコミットが `Cargo.lock` の記録と異なるとき、または記録がないときに、更新として報告する。`Cargo.toml` は書き換えず、新しいコミットは `--install` で実行する `cargo update` がロックファイルに取り込む。`--install` を付けなければ何も書き込まない |
| `rev` | `--include-pinned` を付けても、常に `pinned` としてスキップする |

`tag` の書き換えは、バージョンの書き換えと同じ依存テーブルと `[patch.<registry>]` / `[patch.<registry>.<package>]` に限り、inline table でも複数行テーブルでも単一引用符・二重引用符を保ちます。git 依存には age フィルターも OSV チェックも適用しません。`git ls-remote` に失敗した依存はスキップするだけで、終了コードは変えません。

Cargo の比較演算子による範囲指定では、`>=1.0, <2.0, >=1.0.100` のように 3 個以上の要件をカンマで区切った形式にも対応します。`^1.2.2, <1.5` のようにキャレット・チルダ・ワイルドカードと比較演算子を混ぜた複数要件も、`semver::VersionReq` で有効性を確かめたうえで範囲指定として検出します。上限がなく複数の下限が混在する制約（`>=1.2.3, ^1.3` など）は安全に書き換えられないため、スキップします。

### Go

`go.mod` には `^` や `~` のような範囲指定がなく、完全なバージョンしか書けません。そのため Go では、バージョンの書き方だけでは意図的な固定と判断しません。`// pinned` コメントのない Go の依存は、`--include-pinned` の有無にかかわらず常に更新対象です。更新したくない依存には、行末に `// pinned` コメントを付けてください。付けた依存は `pinned` としてスキップし、`--include-pinned` を指定したときだけ更新します。`// indirect; pinned` のように、コメント内のどこに `pinned` があっても認識します。

Go の `exclude` ディレクティブに書かれたバージョンは、候補から外します（`exclude` の記述自体は書き換えません）。上流モジュールの最新の `go.mod` が `retract` したバージョン（単一のバージョンでも閉区間でも）も候補から外します。タグ付きのバージョンがないモジュールでは Go Proxy の `@latest` へフォールバックします。`.info` の `Time` が省略されている場合は Unix エポックを公開日時として扱うため、公開日が不明なバージョンが `--age` でいつまでも候補から外れることはありません。

Go の `+incompatible` 付きバージョンは、候補に残すかどうかを `go` コマンドと同じ規則で決めます。判定の基準は、SemVer の順で最初に現れる `+incompatible` の直前にある compatible なバージョンです。そのバージョンに本物の `go.mod`（Go Proxy が合成したものではないもの）があれば、それ以降の `+incompatible` をすべて候補から外します。この処理がないと、`github.com/libp2p/go-libp2p` が `v0.49.0` から 2018 年の `v6.0.23+incompatible` へ「更新」され、しかもビルドは通るため、誤りに気づけません。

`replace` ディレクティブで置き換えているモジュールは対象外です。`require` だけを更新すると `replace` が一致しなくなり、ローカルのパッチが気づかないうちに外れるためです。バージョンなしの `replace` はそのモジュールのすべての `require` を、バージョン付きの `replace` は同じバージョンの `require` だけを対象外にします。

`go.mod` では、`) // direct deps` のようにコメントが付いたブロックの終端も通常の終端として扱い、`require` / `replace` / `exclude` ブロックの解析と更新に反映します。`require "golang.org/x/text" "v0.14.0"` のように引用符で囲まれたモジュールパスやバージョンも解析し、引用符を保ったまま更新します。単一行・ブロック形式の `require` を更新するときは、元の改行コード（LF / CRLF）を維持します。

### Ruby

Gemfile の依存宣言は、一般的な Ruby DSL 形式（`gem "rack", "~> 3.0"`）と括弧付きのメソッド呼び出し形式（`gem("rack", "~> 3.0")`）のどちらも解析・更新できます。更新時は元の呼び出し形式を保持します。同じ gem が複数箇所（例: トップレベルと `group :test` ブロックの両方）で宣言されている場合は、書き込み先が曖昧なので書き換えを拒否します。

`gem "pg", ">= 0.18", "< 2.0"` のような Gemfile の複合制約も解析・更新できます。進めるのは包含的な下限（`>= 0.18` のように値そのものを含む下限）だけで、元の引数の個数・順序・引用符の種類・空白・括弧付きの呼び出し形式・行末の `if` / `unless` 修飾子を保ったまま書き戻します。比較の基準は記述順に関係なく包含的な下限なので、`gem "pg", "< 2.0", ">= 0.18"` でも `0.18` を基準に判定します。書き換え後の制約を元の引数の個数へ分割できない場合（引数自体がカンマを含む場合など）は、安全でない書き換えはせずにエラーとして報告します。`gem "rack", "!= 2.2.4"` のような除外制約は、一部だけを更新すると意味が変わるため、スキップします。

Gemfile の次の宣言は対象外です。

- バージョンを書かずに `git:` / `github:` / `bitbucket:` / `gist:` / `path:` / `source:` を指定した gem。RubyGems のレジストリ依存ではないためです。オプションキーは、hash-rocket 形式（`:git => '...'`）も同じように認識します。
- `git ... do` / `github ... do` / `path ... do` / `source ... do` ブロック内の gem。理由は同じです。
- 引数が次の行へ続く宣言（`gem "devise",`）。その行だけではバージョンを決められないためです。

`git:` などを指定していてもバージョンが明示されていれば、Bundler が gemspec のバージョンを検証するための制約として、source オプションを保ったまま解析・更新します。`platforms` / `install_if` のような通常のブロック内の gem は、ほかの gem と同じく処理します。行単位の `group:` / `groups:` オプションは、開発依存かどうかの判定に使います。

Bundler の `git_source(:name) { ... }` で登録したカスタムの git source ショートハンド（例: `gem 'rails', stash: 'forks/rails'`）も、組み込みの `git:` / `github:` と同じ規則で扱います。バージョンのない宣言は、レジストリ外の依存として対象外にします。

### PHP

`composer.json` で更新するのは `require` と `require-dev` です。`replace` / `provide` / `conflict` などのセクションは変更しません。明示的な等価演算子（`=1.2.3` / `==1.2.3`）は、演算子を保ったまま更新します。「等しくない」を `<>` で書いた除外制約（`<>1.2.3` など）は、解析しますが書き換えず、スキップします（[自動更新しない制約](#自動更新しない制約)）。

Composer のプラットフォームパッケージ（`php`、`hhvm`、`ext-*`、`lib-*`、Composer API パッケージなど）は対象外です。`1.0.0 as 1.1.0` のようなインラインエイリアスも対象外です。レジストリの最新版で上書きすると、エイリアスの宣言が壊れてしまうためです。

Composer と Packagist は、`composer/semver` の `VersionParser` に従って 1〜4 セグメントの数値バージョンを有効とみなします。depup も `1.2.3.4`、`^1.0.0.0`、`~3.4.5.6`、`1.0.0.*` のような 4 セグメントまでのバージョンを解析・更新できます。5 セグメント以上は無効なので対象外です。

Composer の修飾子（modifier）は区切り文字を省略でき、`.` / `_` も区切りに使えます（`composer/semver` の正規表現は `[._-]?`）。そのため depup は、`5.0.0alpha3` / `1.0.0.RC1` / `1.0.0_beta1` をプレリリース、`2.2.1p1` / `2.2.1pl1` / `2.2.1patch1` を元のバージョンより新しい修正版（patch alias）として扱います。どちらの形式も現在の Packagist に実在します（`nikic/php-parser` の `5.0.0beta1`、`laminas/laminas-diactoros` のセキュリティパッチ `2.2.1p2`）。

Composer は `~>` 演算子を受け付けない（`Invalid operator "~>"`）ため、PHP では `~>` を含む制約を対象外にし、Composer が読めない制約を書き戻さないようにしています。node-semver は `~>` を有効とするので、Node.js では受け付けます。

### Java（Gradle）

`strictly` / `require` / `prefer` / `reject` を使う Gradle の rich version 宣言は、`implementation("org.slf4j:slf4j-api") { version { ... } }` のような依存ブロック内でも解析します。文字列記法の短縮形でも、完全一致（`group:name:1.2.3!!`）、動的プレフィックス（`group:name:5.3.+!!`）、範囲（`group:name:[1.7, 1.8[!!`）、prefer 付きの strict 範囲（`group:name:[1.7, 1.8[!!1.7.25`）を解析できます。`strictly` または `require` で範囲を、`prefer` で優先するバージョンを指定している場合、depup は範囲を上限制約として維持したまま `prefer` の値を更新します。`reject` に列挙されたバージョンは候補から外し、`2.+` のような動的な reject や `[1.5,1.9)` のような範囲の reject も考慮します。

Gradle の宣言ラッパー `platform(...)` / `enforcedPlatform(...)` / `testFixtures(...)` にも対応しています。`implementation platform('com.google.cloud:libraries-bom:26.1.0')` や `testImplementation(platform("org.junit:junit-bom:5.10.0"))` のような BOM 宣言も解析・更新でき、開発依存かどうかはラッパーの外側にある configuration 名（`implementation` / `testImplementation` など）で判定します。

`ext.<name> = '...'` / `project.ext.<name> = "..."` のドット代入も `ext { ... }` ブロックと同じく変数として解決し、`${Versions.retrofit}` のような修飾付きの参照は最後のセグメントで解決します。同じ変数名が異なる値で複数定義されている場合は変数を解決せず、その変数を参照する依存は対象外になります。別オブジェクトの値を拾う誤更新を避けるためです。

`gradle/*.versions.toml` にある Gradle version catalog は、Java のマニフェストとして検出します。depup は `[libraries]` の `alias = "group:name:version"`、`module = "group:name"`、`group` / `name` / `version`、`version.ref` を解析し、参照先の `[versions]` もその場で更新します。`strictly` / `require` / `prefer` / `reject` / `rejectAll` を含む rich version table は、Gradle のビルドファイルと同じ規則で候補を選びます。`[plugins]` に並ぶのは Gradle のプラグイン ID で、Maven Central の座標とは一致しないため、対象外です。

Gradle の文字列記法では、`:resources@zip` や `@aar` のような classifier / extension の接尾辞を保持します。`//` の行コメントや `/* ... */` のブロックコメントの中だけにある依存宣言は無視します。Gradle version catalog では、バージョンが宣言されている TOML の文字列形式またはテーブル形式を保ったまま更新します。

`-SNAPSHOT` / `.SNAPSHOT` で終わるバージョンを直接指定した依存（`1.2.3-SNAPSHOT`、`1.2.3-SNAPSHOT!!`、`[1.2.3-SNAPSHOT]`）は対象外です。SNAPSHOT は、依存を解決するたびに最新のタイムスタンプ付きビルドを取り直す「動く参照」です。固定のリリース版へ書き換えると、ビルドに使われる中身が気づかないうちに変わります。`.Final` / `.RELEASE` / `-jre` / `-SP1` のような安定版の qualifier が付いたバージョンは、通常どおり更新します。

Gradle の `resolutionStrategy { force ... }` / `constraints { }` / `dependencySubstitution { }` の中に書かれた座標は対象外です。これらのブロックは、他の場所で宣言済みのバージョンを再掲するものです。別々の宣言として扱うと同じ座標が重複し、書き込み先が曖昧になって更新できなくなります。

JVM 系の milestone 版はプレリリースとして扱い、現在のバージョンが安定版なら候補から外します。対象は `4.0.0-M1`、旧 Spring Boot のドット区切り `2.0.0.M1`、省略しない綴りの `-milestone1` です。この扱いがないと、`assertj-core 3.24.2` が `4.0.0-M1` へ、`junit-bom 5.10.0` が `5.13.0-M3` へ、`spring-core 5.3.23` が `7.0.0-M6` へ更新されてしまいます。判定は「`m` の直後が数字」のトークンに限るため、JVM の安定版の qualifier（`.Final`、`-jre`、`-android`、`.RELEASE`、`.GA`、`-SP1`）や `-macos1` のような識別子を誤判定しません。他のプレリリースと同じく、現在のバージョンが milestone の場合は候補に milestone を残すため、次の milestone へ進めます。

### Swift

Swift で対象になるのは、GitHub の URL で指定し、バージョンの要件を書いた依存だけです。GitHub 以外の URL と `branch:` / `revision:` の指定は対象外です。Swift Package Registry の `id:` 依存（`.package(id: "scope.name", ...)`）も、registry API のアダプターが未実装のため対象外です（将来対応する予定です）。

- URL は、HTTPS、scp 形式の SSH（`git@github.com:owner/repo.git`）、標準の SSH（`ssh://git@github.com/owner/repo.git`）、GitHub の SSH over 443（`ssh://git@ssh.github.com:443/owner/repo.git`）を解析します。
- GitHub のタグは `v1.2.3` と `V1.2.3` の両方を認識します。`Package.swift` の version requirement 文字列は、厳格な SemVer（`X.Y.Z`、先頭ゼロなし）として検証します。
- SwiftPM が Semantic Versioning 2.0.0 に従うのに合わせて、プレリリース識別子（`1.0.0-beta.1`）、ビルドメタデータ（`1.0.0+build.123`）、その両方を含む形式（`1.0.0-rc.1+sha.abc`）も解析・更新します。
- `.package(...)` の末尾に `traits: [...]`（SPM 6.1 の Package Traits）や `moduleAliases: [...]` のような引数があっても、version requirement だけを置き換え、ほかの引数は保ちます。
- `//` の行コメントや `/* ... */` のブロックコメントの中にある依存宣言は無視します。

### mise

mise（[jdx/mise](https://mise.jdx.dev)）の設定ファイルに書かれたツールバージョンも、他の言語と同じワークフロー（検出 → 解析 → 判定 → 書き込み）で更新します。バージョン一覧は `mise ls-remote <tool> --json` から取得するため、mise が対応するすべてのバックエンド（core / aqua / ubi / asdf / npm: / cargo: / go: / pipx: など）をそのまま扱えます。

#### 対象ファイル

`mise.toml` / `.mise.toml` / `mise/config.toml` / `.mise/config.toml` / `.config/mise.toml` / `.config/mise/config.toml` / `.tool-versions`

`mise.local.toml`（個人のローカル上書き。通常は gitignore の対象）と `mise.<env>.toml`（環境別の上書き設定）は、特定の環境だけに効く設定です。更新すると、その環境の設定だけが利用者の気づかないうちに変わってしまうため、意図的に読み込みません。

#### バージョン指定の扱い

| 記法 | 例 | 扱い |
|------|-----|------|
| 完全一致 | `node = "26.7.0"` | 最新版へ更新 |
| 前方一致 | `node = "26"` / `"26.7"` | セグメント数を保って更新（`26` → `27`、`26.7` → `26.8`） |
| 明示セレクター | `go = "prefix:1.19"` | `prefix:` を保持して更新（`prefix:1.24`） |
| ベンダー付き | `java = "temurin-21.0.5"` | 同じベンダー内で更新（`temurin-21.0.9`） |
| inline table | `python = { version = "3.13", virtualenv = ".venv" }` | `version` だけ更新し、他のオプションは保持 |
| テーブル形式 | `[tools.terraform]` + `version = "1.15.0"` | 同上 |
| 浮動指定 | `latest` / `lts` / `system` | 固定のバージョンがなく、更新するものがないため対象外 |
| 非バージョン | `ref:master` / `path:./shfmt` / `sub-2:lts` | 対象外 |
| 複数バージョン | `python = ["3.12", "3.13"]` | どれを更新すべきか決められないため対象外 |

`mise ls-remote java` が返す 3000 件超の候補の大半は、`temurin-` / `graalvm-community-` / `zulu-` などのベンダー接頭辞付きです。depup は現在の指定と同じ接頭辞の候補だけを残すため、`temurin-21` を使っているプロジェクトが `zulu-27` に書き換わることはありません。

`[tools]` セクションだけを書き換え、`[settings]` / `[env]` / `[tasks]` / `[alias]` に同名のキーがあっても変更しません。引用符の種類（`"` / `'`）、行末コメント、改行コード（CRLF）、`.tool-versions` の空白の並びはすべて保持します。

#### mise と age フィルター

mise の `[settings] minimum_release_age` が**明示的に書かれている**場合は、pnpm・Bun の `minimumReleaseAge` と同じ[プロジェクトポリシー](#適用される-age-の優先順位)として、CLI の `--age` より優先します。採用するのは設定ファイルに書かれた値だけで、mise の組み込みのデフォルト（24 時間）は採用しません。書き方は「[`minimumReleaseAge` を読み取る設定ファイル](#minimumreleaseage-を読み取る設定ファイル)」を参照してください。

> **注意**: mise の `m` は**分**（humantime 準拠）で、depup CLI の `--age 1m`（1 か月）とは単位が違います。`minimum_release_age = "1m"` は 1 分として扱われます。プロジェクトポリシーは CLI の `--age` より優先されるため、pnpm や Bun にもっと大きな値がなければ、age フィルターは実質的に効かなくなります。1 か月にしたい場合は `"1M"` か `"30d"` と書いてください。

`mise ls-remote` はデフォルトで mise 側の `minimum_release_age` を適用して新しいバージョンを隠しますが、depup は `--minimum-release-age 0` を渡してすべて取得し、age の判定を depup 側に一本化します。`minimum_release_age_excludes` は depup では解釈しないため、設定されている場合は警告を表示します。

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

## コントリビューション

コントリビューションを歓迎します。お気軽にプルリクエストをお送りください。

## ライセンス

[MIT](LICENSE)
