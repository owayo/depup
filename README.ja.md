<p align="center">
  <img src="docs/images/app.png" width="128" alt="depup">
</p>

<h1 align="center">depup</h1>

<p align="center">
  Node.js、Python、Rust、Go、Ruby、PHP、Java、Swift、mise の依存関係を、公開からの経過期間と OSV の脆弱性情報を確かめながらまとめて更新する CLI
</p>

<!-- standard:badges:start -->
<h3 align="center">対応プラットフォーム</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&amp;logoColor=black" alt="Linux">
  <img src="https://img.shields.io/badge/macOS-000000?logo=apple&amp;logoColor=white" alt="macOS">
  <img src="https://img.shields.io/badge/Windows-0078D6" alt="Windows">
</p>

<p align="center">
  <a href="https://github.com/owayo/depup/actions/workflows/ci.yml"><img src="https://github.com/owayo/depup/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/owayo/depup/releases/latest"><img src="https://img.shields.io/github/v/release/owayo/depup" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/owayo/depup" alt="License"></a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>
<!-- standard:badges:end -->

---

depup は、プロジェクトにあるマニフェストを見つけ、レジストリごとに新しいバージョンを調べて、バージョン指定をその場で書き換えます。扱うのは `package.json`、`pyproject.toml`、`Cargo.toml`、`go.mod`、`Gemfile`、`composer.json`、Gradle のビルドファイル、`Package.swift`、mise の設定ファイルです。複数の言語を 1 回の実行でまとめて処理します。

更新先はデフォルトで慎重に選びます。公開から 1 週間以上たったバージョンだけを候補にし、OSV.dev に既知の脆弱性があるバージョンは避けます。固定したバージョンには手を付けず、範囲指定は演算子と上限を保ったまま書き換えます。

`--install` を付けると、更新のあとで各プロジェクトのパッケージマネージャーを実行し、ロックファイルも新しいバージョンに合わせます。

## 機能

- **複数言語対応**: Node.js、Python、Rust、Go、Ruby、PHP、Java、Swift の依存関係に加え、`mise.toml` / `.tool-versions` のツールバージョンも同じワークフローで更新
- **マニフェスト更新**: マニフェストファイル（`package.json` や `Cargo.toml` など）内のバージョン指定を直接更新
- **範囲指定の維持**: バージョン範囲の形式（`^`、`~`、`>=`）を保ったまま、上限を壊さずに更新
- **固定指定の検出**: 意図的に固定したバージョンはデフォルトでスキップ
- **age フィルター**: 公開から N 日（または N 週）以上経過したバージョンにのみ更新（デフォルトは 1 週間）
- **プロジェクトの age 設定**: pnpm・Bun・mise の設定にある最小公開期間を自動適用
- **脆弱性チェック**: 更新先のバージョンを OSV.dev で照会し、既知の脆弱性があるバージョンを避ける（デフォルトで有効）
- **パッケージマネージャーの実行**: `--install` で更新後に各プロジェクトのパッケージマネージャーを実行し、pnpm・uv・mise には age フィルターの値も渡す
- **Bun Catalogs 対応**: `package.json` の Bun `catalog` / `catalogs` 定義を更新
- **モノレポ対応**: `.depup`、Cargo / pnpm / Go のワークスペース、Gradle マルチプロジェクト、入れ子のパッケージごとの install、Tauri プロジェクト
- **複数出力形式**: 各バージョンの公開日時を添えたテキスト（カラー）、JSON、diff

### 対応言語

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

## 動作環境

- **mise**: 実行時には、mise のツールバージョンを更新する場合にのみ必要です（バージョン一覧を `mise ls-remote` から取得するため）。mise がない環境では mise の設定ファイルを読み飛ばし、警告を 1 回表示して他の言語の更新を続けます。
- **パッケージマネージャー**: `--install` は各プロジェクトのパッケージマネージャー（npm、pnpm、uv、Cargo、Bundler、Composer、Gradle など）を実行するため、それらが入っている必要があります。入っていないパッケージマネージャーは install の失敗として扱います。
- **ネットワーク接続**: バージョンは公開レジストリから取得し、脆弱性チェックでは OSV.dev の API に問い合わせます。

## インストール

<!-- standard:install:start -->
### Homebrew (macOS/Linux)

```bash
brew install owayo/depup/depup
```

### winget (Windows)

```powershell
winget install owayo.depup
```

### Cargo

Rust 1.98 以上が必要です。

```bash
cargo install --git https://github.com/owayo/depup --locked
```

### GitHub Releases から

[Releases](https://github.com/owayo/depup/releases/latest) から自分の環境のアーカイブを取得して展開し、`depup` を `PATH` の通った場所に置きます。各リリースには、取得したファイルを確かめるための `SHA256SUMS` も添付しています。

| プラットフォーム | ファイル |
|---|---|
| Linux (x86_64) | `depup-x86_64-unknown-linux-gnu.tar.gz` |
| Linux (ARM64) | `depup-aarch64-unknown-linux-gnu.tar.gz` |
| macOS (Intel) | `depup-x86_64-apple-darwin.tar.gz` |
| macOS (Apple Silicon) | `depup-aarch64-apple-darwin.tar.gz` |
| Windows (x86_64) | `depup-x86_64-pc-windows-msvc.zip` |

macOS でブラウザから取得した場合は、実行の前に隔離属性を外します: `xattr -d com.apple.quarantine depup`。

### ソースから

[mise](https://mise.jdx.dev/) が必要です (Rust のツールチェーンは `mise.toml` で固定しています)。

```bash
git clone https://github.com/owayo/depup.git
cd depup
make install
```

`make install` は `/usr/local/bin` に入れます。場所を変えるときは `INSTALL_PATH` を指定します (例: `make install INSTALL_PATH="$HOME/.local/bin"`)。
<!-- standard:install:end -->

winget でインストールした直後は、PATH の変更を反映させるためにターミナルを開き直してください。ソースから入れたものを取り除くときは、同じ `INSTALL_PATH` を付けて `make uninstall` を実行します。

## 使い方

```bash
depup [OPTIONS] [PATH]
```

`PATH` には処理するディレクトリを指定します（省略時はカレントディレクトリ）。`--node` や `--python` のような言語のオプションで絞らない限り、そこで見つかった対応言語をすべて更新します。

```bash
# すべての更新内容をプレビュー（ドライラン）
depup -n

# Node.js の依存関係のみ更新
depup --node

# lodash と typescript のみ更新
depup --only lodash --only typescript

# react を更新対象から除外
depup --exclude react

# 公開から 2 週間以上経過したバージョンにのみ更新（デフォルトは 1 週間）
depup --age 2w

# diff を表示して更新
depup --diff

# 更新後に npm install などを実行
depup --node --install

# CI/CD 向けに JSON で出力
depup --json
```

Python のプロジェクトと Tauri のプロジェクトでの出力例です。

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

### 更新の基本ルール

depup は、固定されていない依存について公開済みのバージョンを調べ、以下の条件を満たす最新のものを選んでマニフェストを書き換えます。デフォルトの条件は次のとおりです。

- `package.json` の `"1.2.3"` のようにバージョンを 1 つに決めた指定は、意図的な固定（固定指定）とみなし、`--include-pinned` を付けない限り更新しません。Go と mise は例外です（[固定指定と `--include-pinned`](docs/usage.ja.md#固定指定と---include-pinned)）。
- 公開から 1 週間以上経過したバージョンだけを候補にします（[age フィルター](docs/usage.ja.md#age-フィルター)）。
- 既知の脆弱性があるバージョンは避けます（[脆弱性チェック](docs/usage.ja.md#脆弱性チェックosvdev)）。
- 現在のバージョンが安定版なら、プレリリースは提案しません（[候補のバージョン順序とプレリリース](docs/usage.ja.md#候補のバージョン順序とプレリリース)）。
- 範囲指定は形と上限を保ち、下限だけを進めます（[範囲の上限と下限](docs/usage.ja.md#範囲の上限と下限)）。
- メジャーバージョンを上げる更新も許可します。制限するには `--max-change` を使います（[更新幅の制限](docs/usage.ja.md#更新幅の制限--max-change)）。

### 詳しいドキュメント

- [更新候補の絞り込み](docs/usage.ja.md#更新候補の絞り込み): age フィルター、プロジェクトの age 設定、OSV.dev の脆弱性チェック、`--max-change`
- [バージョン指定と書き換え](docs/usage.ja.md#バージョン指定と書き換え): どの指定を固定とみなすか、範囲と書式をどう保つか、どの制約を書き換えないか
- [パッケージマネージャーの実行](docs/usage.ja.md#パッケージマネージャーの実行--install): `--install` がパッケージマネージャーごとに実行するコマンドと、推移的依存に age フィルターがどこまで効くか
- [更新されないとき](docs/usage.ja.md#更新されないとき): `--verbose` で表示されるスキップの理由
- [コマンドラインリファレンス](docs/cli-reference.ja.md): すべてのオプション、テキスト・JSON・diff の出力、終了コード
- [エコシステムとモノレポ](docs/ecosystems.ja.md): ワークスペースと Tauri プロジェクトの扱い、エコシステムごとに depup が読み取る宣言

## 設定

depup は 3 か所から設定を読み取ります。どの設定も、コマンドラインのオプションで指定すれば、その実行に限りグローバル設定ファイルの値より優先されます。例外は age で、プロジェクトに書かれた最小公開期間は `--age` よりも優先されます。

| 設定 | 置き場所 | 用途 |
|---|---|---|
| グローバル設定 | `~/.config/depup/config.toml`（初回実行時に作成） | すべてのプロジェクトに共通する `age`・`osv`・`max_change` のデフォルト |
| モノレポのディレクトリ | プロジェクトのルートの `.depup` | 追加で処理するディレクトリ |
| プロジェクトの age 設定 | pnpm・Bun の設定の `minimumReleaseAge`、mise の設定の `minimum_release_age` | そのプロジェクトの最小公開期間 |

たとえば、どのプロジェクトでも 1 週間ではなく 2 週間待ち、メジャーバージョンを上げる更新をしないようにするには、次のように書きます。

```toml
# ~/.config/depup/config.toml
age = "2w"
max_change = "minor"
```

すべてのキー、`.depup` の書式、不正な値の扱いは [docs/configuration.ja.md](docs/configuration.ja.md) にまとめています。

## 開発

<!-- standard:dev:start -->
[mise](https://mise.jdx.dev/) が必要です。ツールの版は `mise.toml` で固定しています。

```bash
make setup   # ツールチェーン (mise) と依存を取得する
make ci      # CI と同じ検査 (書き換えない)
```

| コマンド | 説明 |
|---|---|
| `make setup` | ツールチェーン (mise) と依存を取得する |
| `make build` | デバッグ版をビルドする |
| `make release` | リリース版をビルドする |
| `make run` | デバッグ版を実行する (引数は ARGS="...") |
| `make test` | テストを実行する |
| `make lint` | clippy を警告ゼロで通す |
| `make fmt` | コードを整形する (書き換える) |
| `make fmt-check` | 整形済みかを確かめる (書き換えない) |
| `make check` | 整形と静的検査 (書き換えない) |
| `make ci` | CI と同じ検査 (書き換えない) |
| `make install` | リリース版を INSTALL_PATH (既定 /usr/local/bin) に入れる |
| `make uninstall` | INSTALL_PATH から取り除く |
| `make clean` | ビルド成果物を消す |

`make` でターゲットの一覧を表示します。リリースは GitHub Actions で行います (**Actions → Release → Run workflow**)。
<!-- standard:dev:end -->

追加のテスト用ターゲット、CI が OS ごとに実行する内容、リリースの手順は [docs/development.ja.md](docs/development.ja.md) で説明しています。

## ライセンス

<!-- standard:license:start -->
[MIT](LICENSE)
<!-- standard:license:end -->
