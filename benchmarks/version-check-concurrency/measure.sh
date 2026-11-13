#!/usr/bin/env bash
# 既定の設定 (age 1w・OSV 有効) で、依存の確認 (版の一覧の取得・判定・OSV の照会) にかかる
# depup の CLI 全体の時間を測る。
#
# 使い方: measure.sh <実装名> <depup の実行ファイル> <回数>
# 出力:   CSV (implementation,scenario,run,seconds,exit_code) を標準出力へ書く。
#
# 作業用のディレクトリは消さずに残す (BENCH_WORK_ROOT か TMPDIR の下)。
# 結果はネットワークとレジストリ・OSV の状態に左右される。変更前と変更後の実行ファイルを
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
root=$(cd "$here/../.." && pwd)
# 作業用のディレクトリは消さずに残す (呼び出し側の一時領域の下に作る)
work=$(mktemp -d "${BENCH_WORK_ROOT:-${TMPDIR:-/tmp}}/depup-bench.XXXXXX")

# 入力は、更新候補ごとに OSV を問い合わせる crates.io 60 件・npm 30 件の合成マニフェストと、
# すべて最新で OSV を問い合わせない depup 自身の Cargo.toml (対照)。
# マニフェストだけを一時ディレクトリへ写し、ほかのマニフェストを検出させない。
mkdir -p "$work/sixty" "$work/npm" "$work/depup-own" "$work/workspace" "$work/home"
cp "$root/benchmarks/crates-io-fetch/sixty-crates.Cargo.toml" "$work/sixty/Cargo.toml"
cp "$root/benchmarks/http-compression/npm-packages.json" "$work/npm/package.json"
cp "$root/Cargo.toml" "$work/depup-own/Cargo.toml"

# workspace: 同じ 60 件を 15 件ずつ 4 つのメンバーに分けた Cargo workspace。
# 依存の総数は sixty と同じで、マニフェストの数だけが違う。
printf '[workspace]\nmembers = ["m1", "m2", "m3", "m4"]\nresolver = "3"\n' > "$work/workspace/Cargo.toml"
deps=$(sed -n '/^\[dependencies\]/,$p' "$work/sixty/Cargo.toml" | grep -E '^[A-Za-z0-9_-]+ = ')
member=0
for name in m1 m2 m3 m4; do
  mkdir -p "$work/workspace/$name"
  {
    printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies]\n' "$name"
    echo "$deps" | sed -n "$((member * 15 + 1)),$((member * 15 + 15))p"
  } > "$work/workspace/$name/Cargo.toml"
  member=$((member + 1))
done

# 利用者のグローバル設定を読ませない (既定の age 1w・OSV 有効で動く)。
# Windows の depup は HOME ではなく利用者のプロファイルから設定を探すので、この指定は効かない。
export HOME="$work/home"

TIMEFORMAT=%R
echo "implementation,scenario,run,seconds,exit_code"
for scenario in sixty workspace npm depup-own; do
  for run in $(seq 1 "$runs"); do
    exit_file="$work/exit"
    seconds=$({ time {
      if "$depup" --dry-run "$work/$scenario" > /dev/null 2>&1; then
        echo 0
      else
        echo $?
      fi > "$exit_file"
    }; } 2>&1)
    echo "$implementation,$scenario,$run,$seconds,$(cat "$exit_file")"
  done
done
