# HTTP の圧縮の測定記録

[English](README.md)

2026-11-06 に測定しました。depup は reqwest の圧縮の機能を有効にしておらず、レジストリの応答をすべて圧縮なしで受け取っていました。npm の版の一覧 (packument) は版の数に比例して大きく、`typescript` は 15.9 MB、`@types/node` は 11.2 MB あります。reqwest の `gzip` 機能を有効にし、`Accept-Encoding: gzip` を送って圧縮した応答を自動で展開するように変更しています。

判定結果は変えていません。両方の入力で、変更前後の `--dry-run --no-osv --json --verbose` の出力を比べ、更新・スキップ・理由・公開日 (秒単位) の一致を確認しました。

## 応答の大きさ

`curl` で同じ URL を圧縮なしと `--compressed` で取得したときの転送量です (2026-11-06)。

| URL | 圧縮なし | gzip | 比率 |
|---|---:|---:|---:|
| `registry.npmjs.org/typescript` | 15,894,727 B | 2,030,480 B | 13% |
| `registry.npmjs.org/@types%2Fnode` | 11,183,996 B | 1,425,600 B | 13% |
| `registry.npmjs.org/react` | 7,112,377 B | 1,386,623 B | 19% |
| `registry.npmjs.org/lodash` | 247,678 B | 47,684 B | 19% |
| `pypi.org/pypi/django/json` | 630,589 B | 140,167 B | 22% |
| `repo.packagist.org/p2/symfony/console.json` | 382,087 B | 48,170 B | 13% |
| `rubygems.org/api/v1/versions/rails.json` | 476,446 B | 58,817 B | 12% |
| `index.crates.io/se/rd/serde` | 164,874 B | 164,874 B | 100% |

npm と PyPI は `Accept-Encoding: br` や `zstd` を送っても圧縮しない応答を返したため、gzip だけを有効にしています。index.crates.io は圧縮した応答を返さないので、crates.io の取得は変わりません。

## 条件

- Windows 11 Home x86_64、AMD Ryzen 7 5700X (8 コア・16 論理プロセッサ)。前後とも同じ家庭用の回線。
- rustc 1.99.1 (`58cf8f9c3`、LLVM 23.1.1)、MSVC ターゲット。両実装を `mise exec -- cargo build --release --locked` でビルド (LTO・codegen-units=1・strip)。
- 変更前はコミット `3ec2ffb1b65c004cbe11fd8e9c7e50ad2e43583f` と同じソース (実行ファイルの SHA-256 `40E91526DC2408FA0702ECC97819A236BD609D38C6D6DB324B2731DAA9249DCA`、12,739,072 バイト)。変更後は reqwest の `gzip` 機能を加え、新しく入る依存を age を満たす版に固定した作業ツリー (`Cargo.lock` の blob `77f270bceaf03ba69e57147f841cae705be113ae`、SHA-256 `99DBB5A43A8A456DC887826B8561EF7E95D825D306816DF6E8C8CD3FA21C7252`、12,797,952 バイト)。
- [measure.sh](measure.sh) を Git Bash 5.3.15 で実行し、bash の `time` が示す実時間を記録しました。順序は before-a → after-a → after-b → before-b で、各ブロックで入力ごとに 5 回、実装ごとに計 10 回です。測定中はビルドやテストを動かしていません。
- 入力はどちらも `--dry-run --no-osv` で実行し、更新候補ごとの OSV の問い合わせを除きます。
  - `npm`: [npm-packages.json](npm-packages.json) (よく使われるパッケージ 30 件、すべてに更新候補あり)。
  - `pypi`: [pypi-packages.toml](pypi-packages.toml) (よく使われるパッケージ 30 件、すべてに更新候補あり)。
- Windows の depup は `HOME` ではなく利用者のプロファイルからグローバル設定を読みます。測定時の設定は自動生成された既定値 (`age = "1w"`、`osv = true`) です。
- 時間にはプロセスの起動、TLS の接続確立、レジストリの応答、JSON の解析を含みます。回線が速いほど転送量の削減の効果は小さくなります。数値は同じ時間帯に交互に測った前後の比較として読んでください。

## 結果

時間は各実装 10 回の中央値で、単位は秒です。最小・最大値は [comparison.csv](comparison.csv) にあります。短縮率は `(1 - 変更後 / 変更前) * 100` です。

| 入力 | 変更前 (秒) | 変更後 (秒) | 短縮率 |
|---|---:|---:|---:|
| npm (30 件) | 4.870 (4.501〜6.274) | 2.728 (2.541〜5.346) | 43.97% |
| pypi (30 件) | 2.058 (1.710〜5.131) | 1.083 (0.744〜6.721) | 47.36% |

どちらの入力でも、変更後の 10 回のうち 9 回は変更前の最小値より短くなりました。残りの 1 回 (npm 5.346 秒、pypi 6.721 秒) と変更前の pypi の 5.131 秒は、ほかの回より大きく外れています。原因は切り分けていません。全 40 回とも終了コードは 0 でした。実行ファイルは 58,880 バイト (0.46%) 大きくなりました。

生データ: [before-a](before-a.csv)、[after-a](after-a.csv)、[after-b](after-b.csv)、[before-b](before-b.csv)。

## 対象外

- メモリの使用量は測っていません。展開後の応答の大きさは変わらないため、JSON の解析に使うメモリは変わらない見込みですが、確認していません。
- Packagist と RubyGems は応答の大きさだけを確かめ、CLI 全体の時間は測っていません。
