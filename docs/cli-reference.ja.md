# コマンドラインリファレンス

depup の構文、すべてのオプション、使用例、出力形式と終了コードをまとめています。更新先のバージョンの選び方は[使い方ガイド](usage.ja.md)で説明しています。

## 基本構文

```bash
depup [OPTIONS] [PATH]
```

`PATH` には処理するディレクトリを指定します（省略時はカレントディレクトリ）。`--cd` を指定した場合は、先にそのディレクトリへ移動してから `PATH` を解釈します。

## オプション

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

## 使用例

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

## 出力と終了コード

### 進捗表示

<p align="center">
  <img src="images/scanning.png" alt="depup のスキャン中の表示">
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
