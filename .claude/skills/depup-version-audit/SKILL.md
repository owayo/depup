---
name: depup-version-audit
description: >
  depup プロジェクトの各言語パーサ（Node/Python/Rust/Go/Ruby/PHP/Java/Swift）が
  公式仕様のバージョン指定構文をどこまで対応しているかを網羅的に監査するスキル。
  実装コードと公式ドキュメントを突き合わせ、対応状況を分類・報告し、不足分の実装とテストを追加する。
  トリガー: "バージョン監査", "version audit", "仕様チェック", "網羅チェック",
  "バージョン指定の対応状況", "spec coverage", "parser audit"
---

# depup Version Specification Audit

depup の各言語パーサが公式仕様をどこまでカバーしているかを監査し、
不足分を実装・テストするワークフロー。

## Workflow

### Phase 1: 公式仕様の収集

各言語の公式バージョン指定仕様を確認する。
詳細は [references/version_specs.md](references/version_specs.md) を参照。

### Phase 2: 実装コードの読み取り

以下のファイルを読み取り、対応している構文をリストアップする:

| 言語 | パーサ | マニフェスト |
|------|--------|-------------|
| Node.js | `src/parser/node.rs` | `src/manifest/package_json.rs` |
| Python | `src/parser/python.rs` | `src/manifest/pyproject_toml.rs` |
| Rust | `src/parser/rust.rs` | `src/manifest/cargo_toml.rs` |
| Go | `src/parser/go.rs` | `src/manifest/go_mod.rs` |
| Ruby | `src/parser/ruby.rs` | `src/manifest/gemfile.rs` |
| PHP | `src/parser/php.rs` | `src/manifest/composer_json.rs` |
| Java | `src/parser/java.rs` | `src/manifest/gradle.rs` |
| Swift | `src/parser/swift.rs` | `src/manifest/package_swift.rs` |

補助ファイル:
- `src/domain/version_spec.rs` - VersionSpecKind enum
- `src/update/mod.rs` - Range 上限値抽出ロジック
- `src/update/version_info.rs` - バージョン比較ロジック

### Phase 3: 突き合わせと分類

公式仕様の各構文を以下の3カテゴリに分類する:

1. **対応済み** - パーサが正しくパースし、VersionSpecKind にマッピングされている
2. **未対応（実害あり）** - 実際のプロジェクトで使われる構文だが、パースできないか誤分類される
3. **未対応（軽微）** - 極めて稀な構文、または depup の目的上対応不要なもの

### Phase 4: レポート出力

以下の形式で結果を出力する:

```markdown
## [言語名]

### 対応済み
- `構文例` → VersionSpecKind::Xxx (`ファイル:行番号`)

### 未対応（実害あり）
- `構文例` - 公式仕様の説明
  - 影響: どのような場合に問題になるか
  - 対応案: 実装方針

### 未対応（軽微）
- `構文例` - 理由（depup の目的上不要、等）
```

### Phase 5: 実装とテスト

未対応（実害あり）の項目について:
1. パーサに対応を追加
2. 既存テストに追加テストケースを記述
3. `cargo test` を実行して全テスト通過を確認
4. `cargo clippy -- -D warnings` で警告がないことを確認

## 判断基準

depup はバージョン指定を「パースして分類」→「レジストリから最新を取得」→「更新判定」→「マニフェスト書き換え」するツールである。
そのため:

- **パースして VersionSpecKind に分類できること**が最重要
- Range の上限値抽出（`extract_upper_bound`）が正しく動くこと
- `format_updated()` で prefix/suffix を保持して新バージョンを書き出せること
- セマンティクス（caret の厳密な互換性範囲計算など）は depup の範囲外

## depup が意図的にスキップする構文

- npm の `workspace:*`, `file:`, `git://` 等のプロトコル参照
- Swift の `branch:`, `revision:`, `path:` 依存、Swift Package Registry の `id:` 依存（レジストリアダプタ未実装）
- Go の `replace` ディレクティブ
- Composer の platform packages (`php`, `ext-*`)、インラインエイリアス (`1.0.0 as 1.1.0`)
- Gradle の build.gradle 内 version catalog アクセサ (`libs.xxx`)。実体は `gradle/*.versions.toml` 側で更新するため、build.gradle 側は書き換えない
- Cargo / Poetry の path 依存、`[tool.uv.sources]` の workspace / git / path / url 指定
- pnpm-workspace.yaml の catalogs（`package.json` 側の `catalog:` 参照ごとスキップ）
- Poetry のマルチプル制約配列形式（`foo = [{version = "<=1.9", python = "..."}, ...]`）
- Python の環境マーカー（パース後に除去）

> **注**: `platform()` / `enforcedPlatform()` / `testFixtures()` は以前スキップ対象だったが、
> BOM は推移依存のバージョンを一括決定する要の宣言のため**対応済み**に変更した。
