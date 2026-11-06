#!/usr/bin/env bash
# npm と PyPI から版の一覧を取る時間を含め、depup の CLI 全体の所要時間を測る。
#
# 使い方: measure.sh <実装名> <depup の実行ファイル> <回数>
# 出力:   CSV (implementation,scenario,run,seconds,exit_code) を標準出力へ書く。
#
# 結果はネットワークとレジストリの状態に左右される。変更前と変更後の実行ファイルを
# before-a → after-a → after-b → before-b の順に交互に測り、時間帯の差をならす。
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $0 <implementation> <depup> <runs>" >&2
  exit 64
fi
implementation=$1
depup=$2
runs=$3
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# 入力は npm 30 件と PyPI 30 件の合成マニフェスト。マニフェストだけを一時ディレクトリへ
# 写し、ほかのマニフェストを検出させない。
mkdir -p "$work/npm" "$work/pypi" "$work/home"
cp "$here/npm-packages.json" "$work/npm/package.json"
cp "$here/pypi-packages.toml" "$work/pypi/pyproject.toml"

# 利用者のグローバル設定を読ませない (既定の age 1w で動く)。
# Windows の depup は HOME ではなく利用者のプロファイルから設定を探すので、この指定は効かない。
export HOME="$work/home"

TIMEFORMAT=%R
echo "implementation,scenario,run,seconds,exit_code"
for scenario in npm pypi; do
  for run in $(seq 1 "$runs"); do
    exit_file="$work/exit"
    # 更新候補ごとの OSV の問い合わせを除き、版の一覧の取得を比べる
    seconds=$({ time {
      if "$depup" --dry-run --no-osv "$work/$scenario" > /dev/null 2>&1; then
        echo 0
      else
        echo $?
      fi > "$exit_file"
    }; } 2>&1)
    echo "$implementation,$scenario,$run,$seconds,$(cat "$exit_file")"
  done
done
