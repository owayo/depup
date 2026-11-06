# crates.io の取得の測定記録

[English](README.md)

2026-11-06 に測定しました。従来は crates.io の版の一覧を API (`https://crates.io/api/v1/crates/{name}`) だけから取り、実行全体で 1 リクエスト/秒に抑えていました。依存 1 件ごとに約 1 秒待つため、Rust のプロジェクトでは依存の数にほぼ比例して時間がかかっていました。cargo と同じ sparse index (`https://index.crates.io/`) を先に引き、マニフェストごとに最大 4 件を並列に取るように変更しています。index で一覧を確定できない場合と、公開者で age を免除する設定 (`[age_exempt]`) がある場合は、従来どおり API を使います。

判定結果は変えていません。両方の入力で、変更前後の `--dry-run --json --verbose` の出力を比べ、更新・スキップ・理由・公開日 (秒単位) の一致を確認しました。

## 条件

- Windows 11 Home x86_64、AMD Ryzen 7 5700X (8 コア・16 論理プロセッサ)。前後とも同じ家庭用の回線。
- rustc 1.99.1 (`58cf8f9c3`、LLVM 23.1.1)、MSVC ターゲット。両実装を `mise exec -- cargo build --release --locked` でビルド (LTO・codegen-units=1・strip)。依存とツールチェーンは同じです (`Cargo.lock` の blob `6c313e5350215b41abd3b788a59493a3f0cf7d05`)。
- 変更前はコミット `3ee5b038c131cad0ee14b5ded4794f154b84833e` (実行ファイルの SHA-256 `674518C439522CED50304C2539B4DBB6A7BD5080B13EBD8A6D71BB7874297EE7`)。変更後は同じコミットに sparse index の変更を加えた作業ツリー (SHA-256 `40E91526DC2408FA0702ECC97819A236BD609D38C6D6DB324B2731DAA9249DCA`)。
- [measure.sh](measure.sh) を Git Bash 5.3.15 で実行し、bash の `time` が示す実時間を記録しました。順序は before-a → after-a → after-b → before-b で、各ブロックで入力ごとに 3 回、実装ごとに計 6 回です。測定中はビルドやテストを動かしていません。
- 入力:
  - `depup-own`: depup 自身の `Cargo.toml` (直接依存 22 件、すべて最新)。`--dry-run` と既定の設定 (age 1w、OSV 有効) で実行します。最新の依存には OSV を問い合わせません。
  - `sixty`: [sixty-crates.Cargo.toml](sixty-crates.Cargo.toml) (よく使われる crate 60 件、すべてに更新候補あり)。`--dry-run --no-osv` で実行し、更新候補ごとの OSV の問い合わせを除きます。
- Windows の depup は `HOME` ではなく利用者のプロファイルからグローバル設定を読みます。測定時の設定は自動生成された既定値 (`age = "1w"`、`osv = true`) です。
- 時間にはプロセスの起動、TLS の接続確立、レジストリと CDN の応答を含みます。時間帯や回線で変わるため、数値は同じ時間帯に交互に測った前後の比較として読んでください。

## 結果

時間は各実装 6 回の中央値で、単位は秒です。最小・最大値は [comparison.csv](comparison.csv) にあります。短縮率は `(1 - 変更後 / 変更前) * 100` です。

| 入力 | 変更前 (秒) | 変更後 (秒) | 短縮率 |
|---|---:|---:|---:|
| depup-own (直接依存 22 件) | 23.203 (22.377〜23.692) | 0.702 (0.595〜0.867) | 96.97% |
| sixty (依存 60 件、`--no-osv`) | 62.620 (61.676〜65.053) | 1.422 (0.906〜3.336) | 97.73% |

どちらの入力でも前後の分布は重なりません。変更前は依存 1 件あたり約 1 秒で、API の間隔待ちが所要時間のほとんどを占めていました。変更後は index の取得が並列になり、依存数の影響が小さくなりました。変更後の sixty の最大値 3.336 秒はほかの 5 回 (0.906〜1.938 秒) より長く、原因は切り分けていません。全 24 回とも終了コードは 0 でした。

生データ: [before-a](before-a.csv)、[after-a](after-a.csv)、[after-b](after-b.csv)、[before-b](before-b.csv)。

## 対象外

- 公開者で age を免除する設定 (`[age_exempt]`) がある実行は、従来どおり API を使うため変わりません。
- install 後の Cargo.lock の age 監査は、以前から sparse index を最大 8 件並列で使っています。この測定は `--dry-run` の確認までで、install と監査を含みません。
- index.crates.io は gzip などで圧縮した応答を返さないため、HTTP の圧縮はこの経路の転送量を変えません。
