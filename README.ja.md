<p align="center">
  <img src="docs/images/app.png" width="128" alt="depup">
</p>

<h1 align="center">depup</h1>

<p align="center">
  マルチ言語依存関係アップデートCLIツール
</p>

<p align="center">
  <a href="README.md">English</a>
</p>

---

<p align="center">
  <img src="docs/images/output.png" width="600" alt="depup output">
</p>

## 特徴

- **マルチ言語対応**: Node.js, Python, Rust, Go
- **マニフェスト更新**: マニフェストファイル内のバージョン指定を直接更新
- **スマートバージョン処理**: バージョン範囲形式（^, ~, >=）を維持
- **固定バージョン検出**: 意図的に固定されたバージョンはデフォルトでスキップ
- **エイジフィルター**: N日/週前以降にリリースされたバージョンのみに更新
- **pnpm連携**: pnpm設定の `minimumReleaseAge` を自動適用
- **モノレポ対応**: pnpmワークスペースとTauriプロジェクト
- **リリース日表示**: 各バージョンのリリース日時を表示
- **複数出力形式**: テキスト（カラー）、JSON、diff

## 対応言語

| 言語 | マニフェスト | ロックファイル |
|------|-------------|---------------|
| Node.js | package.json | package-lock.json, pnpm-lock.yaml, yarn.lock |
| Python | pyproject.toml | uv.lock, poetry.lock |
| Rust | Cargo.toml | Cargo.lock |
| Go | go.mod | go.sum |

## インストール

### ソースから

```bash
git clone https://github.com/owayo/depup.git
cd depup
cargo install --path .
```

### Cargoを使用

```bash
cargo install depup
```

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
| `--dry-run` | `-n` | 変更せずに更新内容を表示 |
| `--verbose` | | 詳細出力を有効化 |
| `--quiet` | `-q` | 最小限の出力 |
| `--node` | | Node.jsの依存関係のみ更新 |
| `--python` | | Pythonの依存関係のみ更新 |
| `--rust` | | Rustの依存関係のみ更新 |
| `--go` | | Goの依存関係のみ更新 |
| `--exclude <PKG>` | | 特定パッケージを除外（複数指定可） |
| `--only <PKG>` | | 特定パッケージのみ更新（複数指定可） |
| `--include-pinned` | | 固定バージョンも更新対象に含める |
| `--age <DURATION>` | | 最小リリース経過期間（例: 2w, 10d, 1m） |
| `--json` | | JSON形式で出力 |
| `--diff` | | diff形式で変更を表示 |
| `--install` | | 更新後にパッケージマネージャのinstallを実行 |
| `--version` | `-v` | バージョンを表示 |
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

# CI/CD用にJSON出力
depup --json

# 更新後にnpm installを実行
depup --node --install
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

`--include-pinned` で固定バージョンも更新対象にできます。

### 範囲形式の維持

depupは元のバージョン範囲形式を維持します：

```
"^1.2.3" → "^2.0.0"  （キャレット維持）
"~1.2.3" → "~1.3.0"  （チルダ維持）
">=1.0.0" → ">=2.0.0" （範囲維持）
```

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

### pnpmワークスペース

depupは `pnpm-workspace.yaml` を検出し、全てのワークスペースパッケージを処理します。

### Tauriプロジェクト

depupはTauriプロジェクトの `src-tauri/Cargo.toml` を自動検出します。

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
