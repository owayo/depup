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
- **マニフェスト更新**: マニフェストファイル内のバージョン指定を直接更新
- **スマートバージョン処理**: バージョン範囲形式（^, ~, >=）を維持しつつ、上限を壊さずに更新
- **固定バージョン検出**: 意図的に固定されたバージョンはデフォルトでスキップ
- **エイジフィルター**: N日/週前以降にリリースされたバージョンのみに更新
- **pnpm連携**: pnpm設定の `minimumReleaseAge` を自動適用
- **モノレポ対応**: pnpmワークスペースとTauriプロジェクト
- **リリース日表示**: 各バージョンのリリース日時を表示
- **複数出力形式**: テキスト（カラー）、JSON、diff

## 対応言語

| 言語 | マニフェスト | レジストリ | ロックファイル |
|------|-------------|----------|---------------|
| <img src="https://img.shields.io/badge/-339933?logo=nodedotjs&logoColor=white" height="16"> Node.js | package.json | npm | package-lock.json, pnpm-lock.yaml, yarn.lock |
| <img src="https://img.shields.io/badge/-3776AB?logo=python&logoColor=white" height="16"> Python | pyproject.toml | PyPI | uv.lock, rye.lock, poetry.lock |
| <img src="https://img.shields.io/badge/-000000?logo=rust&logoColor=white" height="16"> Rust | Cargo.toml | crates.io | Cargo.lock |
| <img src="https://img.shields.io/badge/-00ADD8?logo=go&logoColor=white" height="16"> Go | go.mod | Go Proxy | go.sum |
| <img src="https://img.shields.io/badge/-CC342D?logo=ruby&logoColor=white" height="16"> Ruby | Gemfile | RubyGems | Gemfile.lock |
| <img src="https://img.shields.io/badge/-777BB4?logo=php&logoColor=white" height="16"> PHP | composer.json | Packagist | composer.lock |
| <img src="https://img.shields.io/badge/-ED8B00?logo=openjdk&logoColor=white" height="16"> Java | build.gradle, build.gradle.kts | Maven Central | gradle.lockfile |
| <img src="https://img.shields.io/badge/-F05138?logo=swift&logoColor=white" height="16"> Swift | Package.swift | GitHub Tags | Package.resolved |

## 動作要件

- **OS**: macOS, Linux, Windows
- **Rust**: 1.85以上（ソースからビルドする場合）

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
| `--exclude <PKG>` | | 特定パッケージを除外（複数指定可） |
| `--only <PKG>` | | 特定パッケージのみ更新（複数指定可） |
| `--include-pinned` | | 固定バージョンも更新対象に含める |
| `--age <DURATION>` | | 最小リリース経過期間（例: 2w, 10d, 1m） |
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

# 2週間以上経過したパッケージのみ更新
depup --age 2w

# PythonとRustのみ更新
depup --python --rust

# Java（Gradle）の依存関係のみ更新
depup --java

# Swift（Package.swift）の依存関係のみ更新
depup --swift

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
> **注意**: `gem "pg", ">= 0.18", "< 2.0"` や `gem "rack", "!= 2.2.4"` のような Gemfile の複合制約・除外制約は解析対象ですが、自動では書き換えません。制約の一部だけを更新すると意味が壊れるため、unsafe な編集は適用せずに報告します。
>
> **注意**: バージョンなしで `git:` / `github:` / `bitbucket:` / `gist:` / `path:` / `source:` を指定した Gemfile 依存は、RubyGems のレジストリ制約へ変換せずにスキップします。行単位の `group:` / `groups:` オプションは開発依存の判定に使います。
>
> **注意**: `alias = { package = "actual-crate", version = "1" }` のような Cargo のリネーム依存は、実パッケージ名で取得し、マニフェスト上のキーへ書き戻します。`--only` / `--exclude` はどちらの名前でも指定できます。
>
> **注意**: Composer の platform package（`php`, `hhvm`, `ext-*`, `lib-*`, Composer API パッケージなど）は更新対象から除外します。

### 範囲形式の維持

depupは元のバージョン範囲形式を維持します：

```
"^1.2.3" → "^2.0.0"  （キャレット維持）
"~1.2.3" → "~1.3.0"  （チルダ維持）
">=1.0.0" → ">=2.0.0" （範囲維持）
"requests (>=2.28,<3); python_version < '3.12'" → "requests (>=2.31,<3); python_version < '3.12'" （PEP 508 の括弧とマーカーを維持）
"coverage [toml] >=7,<8" → "coverage [toml] >=7.6,<8" （PEP 508 extras の空白を維持）
"1.x" → "2.x" （ワイルドカード形式を維持）
"1.x.x" → "2.x.x" （複数のワイルドカード位置を維持）
"1.2.*" → "1.3.*" （ワイルドカード形式を維持）
"v1.*" → "v2.*" （先頭の `v` を維持）
"5.3.+" → "5.4.+" （Gradle プレフィックスを維持）
"1.2.3!!" → "2.0.0!!" （Gradle strict を維持）
"[1.0]" → "[2.0]" （Maven Hard requirement を維持）
"[1.2.3.Final]" → "[1.3.0]" （qualifier 付き Maven Hard requirement）
group = "com.google.guava", name = "guava", version = "32.1.2-jre" → version = "33.4.0-jre" （Gradle Kotlin map 記法）
```

`"*"`、npm の dist-tag（`"latest"` など）、Gradle の動的指定（`"latest.release"`、`"latest.integration"`、`"latest.milestone"`、ユーザ定義 `latest.<status>` など全般）のような完全浮動指定は、厳密バージョンへ変質させないため更新対象から除外されます。

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

依存関係に上限付きレンジ（例：`>=3.5.0,<4.0.0`、`>=1.0,<=2.0`、`4.0.0...4.9.9`）がある場合、depup は：
- 上限を超えるバージョンを**提案しません**
- 包含的上限（`<=`、`...`）は上限値自体を候補に含めます
- マニフェストファイル内の元の制約形状を**維持**します
- 指定範囲内で互換な最新バージョンに合わせて、**下限側だけを更新**します

安全に書き換えられない制約は、部分更新せずにスキップします。例として npm/Composer の OR 制約（`^1 || ^2`）、除外専用制約（`!=1.2.3`）、上限のみの制約（`<4.0.0`、`<=2.0`）、厳密な下限制約（`>1.0.0`）、下限を持たない Maven 形式レンジ（`(,2.0]`）、Maven の排他的下限レンジ（`]1.0,2.0[`）などが該当します。

JSON マニフェストでは、depup が解析対象にする依存セクションだけを書き換えます。`package.json` の `overrides`、`composer.json` の `replace` / `provide` / `conflict` などは変更しません。

Swift の GitHub 依存では、タグの接頭辞 `v1.2.3` と `V1.2.3` の両方を認識します。
また、`Package.swift` では `//` 行コメントや `/* ... */` ブロックコメント内に書かれた依存宣言は解析対象から除外します。

`go.mod` では、`) // direct deps` のようなコメント付きブロック終端も通常のブロック終端として扱い、`require` / `replace` / `exclude` ブロックのパースと更新に反映します。

## エイジフィルター

`--age` オプションは、一定期間リリースされているバージョンのみに更新することで安定性を確保します：

```bash
# 2週間以上経過したバージョンのみに更新
depup --age 2w

# 10日以上経過したバージョンのみに更新
depup --age 10d

# 1ヶ月以上経過したバージョンのみに更新
depup --age 1m
```

### pnpm連携

depupはpnpm設定から `minimumReleaseAge` を自動的に読み取ります：

**優先順位：**
1. CLI `--age` フラグ（最優先）
2. `.npmrc`（`minimum-release-age=10d`）
3. `pnpm-workspace.yaml`（`minimumReleaseAge: 14400` 分単位）
4. `package.json`（`pnpm.settings.minimumReleaseAge`）

### Swift とエイジフィルター

GitHub Tags API はタグのリリース日時を返しません。そのため Swift パッケージは `--age` 指定時もエイジフィルターの対象外として扱われます（更新対象に含まれます）。

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

- `#` 以降はコメント（行頭・インライン両対応）
- 空行は無視
- パスは `.depup` ファイルの配置ディレクトリからの相対パス
- 存在しないディレクトリは警告してスキップ
- ルートディレクトリは常にスキャン対象に含まれる

### pnpmワークスペース

depupは `pnpm-workspace.yaml` を検出し、全てのワークスペースパッケージを処理します。

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
