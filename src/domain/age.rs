//! リリース経過時間 (age) の共通ユーティリティ
//!
//! `--age` / `minimumReleaseAge` / グローバル設定など複数の入口から
//! `Duration` が入ってくるため、範囲検証とカットオフ時刻の算出をここへ集約する。
//!
//! chrono の `DateTime` が表現できる年範囲は ±262143 年しかないのに対し、
//! `chrono::Duration::from_std` は `i64::MAX / 1000` 秒 (約 2.9 億年) まで
//! 通してしまう。この差の帯域を素通しすると `Utc::now() - duration` が
//! 内部の `expect("`DateTime - TimeDelta` overflowed")` で panic するため、
//! 入力側 (`checked_age`) と消費側 (`cutoff_from`) の二層で防ぐ。

use chrono::{DateTime, Utc};
use std::time::Duration;

/// 受け付ける age の上限 (10 万年)。
///
/// chrono の `DateTime` 範囲 (±262143 年) より十分小さく、かつ現実的な
/// 設定値をすべて包含する値にしている。
pub const MAX_AGE_SECS: u64 = 100_000 * 365 * 24 * 60 * 60;

/// 秒数を age の `Duration` へ変換する。上限を超える場合は `None`。
///
/// 設定ファイル由来の巨大値 (`minimum-release-age=999999999999` など) を
/// ここで弾き、後段のカットオフ算出が panic しないようにする。
pub fn checked_age(seconds: u64) -> Option<Duration> {
    (seconds <= MAX_AGE_SECS).then(|| Duration::from_secs(seconds))
}

/// 分数を age の `Duration` へ変換する。オーバーフロー・上限超過は `None`。
///
/// pnpm / npm の `minimum-release-age` は分単位で指定される。
pub fn checked_age_from_minutes(minutes: u64) -> Option<Duration> {
    minutes.checked_mul(60).and_then(checked_age)
}

/// 基準時刻から `age` だけ遡ったカットオフ時刻を返す。
///
/// `age` が chrono の表現範囲を超える場合は `None` を返す (panic しない)。
/// 呼び出し側は `None` を「age 制約を適用しない」として扱う。
pub fn cutoff_from(now: DateTime<Utc>, age: Duration) -> Option<DateTime<Utc>> {
    let delta = chrono::Duration::from_std(age).ok()?;
    now.checked_sub_signed(delta)
}

/// 現在時刻から `age` だけ遡ったカットオフ時刻を返す。
pub fn cutoff_now(age: Duration) -> Option<DateTime<Utc>> {
    cutoff_from(Utc::now(), age)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checked_age_accepts_realistic_values() {
        assert_eq!(checked_age(0), Some(Duration::from_secs(0)));
        // 1 週間
        assert_eq!(checked_age(604_800), Some(Duration::from_secs(604_800)));
        // 上限ちょうどは受け付ける
        assert_eq!(
            checked_age(MAX_AGE_SECS),
            Some(Duration::from_secs(MAX_AGE_SECS))
        );
    }

    #[test]
    fn test_checked_age_rejects_over_limit() {
        assert_eq!(checked_age(MAX_AGE_SECS + 1), None);
        assert_eq!(checked_age(u64::MAX), None);
    }

    #[test]
    fn test_checked_age_from_minutes() {
        assert_eq!(
            checked_age_from_minutes(10_080),
            Some(Duration::from_secs(604_800))
        );
        // u64 の乗算オーバーフロー
        assert_eq!(checked_age_from_minutes(u64::MAX), None);
        // 乗算は通るが上限を超える値 (約 190 万年)
        assert_eq!(checked_age_from_minutes(999_999_999_999), None);
    }

    #[test]
    fn test_cutoff_from_normal_range() {
        let now: DateTime<Utc> = "2026-08-07T00:00:00Z".parse().unwrap();
        let cutoff = cutoff_from(now, Duration::from_secs(604_800)).unwrap();
        assert_eq!(cutoff.to_rfc3339(), "2026-07-31T00:00:00+00:00");
    }

    #[test]
    fn test_cutoff_from_does_not_panic_on_huge_age() {
        let now: DateTime<Utc> = "2026-08-07T00:00:00Z".parse().unwrap();
        // chrono::Duration::from_std は通るが DateTime の範囲を超える帯域
        // (約 190 万年)。panic せず None を返すこと。
        let huge = Duration::from_secs(999_999_999_999 * 60);
        assert_eq!(cutoff_from(now, huge), None);
        // from_std 自体が失敗する帯域 (約 2.9 億年超)
        assert_eq!(cutoff_from(now, Duration::from_secs(u64::MAX)), None);
    }
}
