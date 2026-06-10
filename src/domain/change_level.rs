//! バージョン変更の semver レベル
//!
//! `--max-change` フラグで上限を指定するために使う。
//! 順序は `Patch < Minor < Major`。

use serde::{Deserialize, Serialize};
use std::fmt;

/// semver 互換性レベル。`Patch < Minor < Major` の順序を持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeLevel {
    /// パッチレベルの変更 (1.0.0 → 1.0.1)
    Patch,
    /// マイナーレベルの変更 (1.0.0 → 1.1.0)
    Minor,
    /// メジャーレベルの変更 (1.0.0 → 2.0.0)
    Major,
}

impl ChangeLevel {
    /// `"patch" / "minor" / "major"` を受け付ける (大文字小文字無視)。
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "patch" => Ok(ChangeLevel::Patch),
            "minor" => Ok(ChangeLevel::Minor),
            "major" => Ok(ChangeLevel::Major),
            other => Err(format!(
                "invalid change level '{}' (expected: patch / minor / major)",
                other
            )),
        }
    }

    /// 文字列リテラル表現。
    pub fn as_str(&self) -> &'static str {
        match self {
            ChangeLevel::Patch => "patch",
            ChangeLevel::Minor => "minor",
            ChangeLevel::Major => "major",
        }
    }

    /// 古いバージョンと新しいバージョンから変更レベルを算出する。
    ///
    /// 数値コア部分のみ比較する (`v` プレフィックス、`-rc.1` / `+build` などの
    /// プレリリース・ビルドメタデータは無視)。欠落セグメントは 0 として比較する
    /// (例: `"1"` と `"1.0.1"` の差は Patch)。どちらかがパースできない場合や、
    /// 数値部分が完全一致する場合は `None`。
    pub fn from_versions(old: &str, new: &str) -> Option<Self> {
        let old_p = split_core(old);
        let new_p = split_core(new);
        if old_p.is_empty() || new_p.is_empty() {
            return None;
        }
        // 欠落セグメントは 0 として比較する ("1" == "1.0.0")
        let seg = |parts: &[u64], idx: usize| parts.get(idx).copied().unwrap_or(0);
        if seg(&old_p, 0) != seg(&new_p, 0) {
            Some(ChangeLevel::Major)
        } else if seg(&old_p, 1) != seg(&new_p, 1) {
            Some(ChangeLevel::Minor)
        } else if seg(&old_p, 2) != seg(&new_p, 2) {
            Some(ChangeLevel::Patch)
        } else {
            None
        }
    }
}

impl fmt::Display for ChangeLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 比較用の数値コア先頭 3 セグメントを取り出す。
///
/// 抽出規則は `compare_versions` と同じ `crate::update::numeric_core` を共用する:
/// `v` プレフィックス / ビルドメタデータ / エポック / プレリリースを除き、
/// 各セグメントは先頭の数値プレフィックスのみを取る (例: `"0rc1"` → 0)。
/// 数値が全く無いセグメント (qualifier 等) 以降は無視するため、
/// 非数値セグメントを読み飛ばして詰めることによる位置ずれ比較は起きない。
fn split_core(s: &str) -> Vec<u64> {
    crate::update::numeric_core(s.trim())
        .into_iter()
        .take(3)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ordering() {
        assert!(ChangeLevel::Patch < ChangeLevel::Minor);
        assert!(ChangeLevel::Minor < ChangeLevel::Major);
        assert!(ChangeLevel::Patch < ChangeLevel::Major);
    }

    #[test]
    fn test_parse_lowercase() {
        assert_eq!(ChangeLevel::parse("patch"), Ok(ChangeLevel::Patch));
        assert_eq!(ChangeLevel::parse("minor"), Ok(ChangeLevel::Minor));
        assert_eq!(ChangeLevel::parse("major"), Ok(ChangeLevel::Major));
    }

    #[test]
    fn test_parse_case_insensitive_and_trim() {
        assert_eq!(ChangeLevel::parse("PATCH"), Ok(ChangeLevel::Patch));
        assert_eq!(ChangeLevel::parse(" Minor "), Ok(ChangeLevel::Minor));
    }

    #[test]
    fn test_parse_invalid() {
        assert!(ChangeLevel::parse("nope").is_err());
        assert!(ChangeLevel::parse("").is_err());
        assert!(ChangeLevel::parse("Patch1").is_err());
    }

    #[test]
    fn test_from_versions_patch() {
        assert_eq!(
            ChangeLevel::from_versions("1.2.3", "1.2.4"),
            Some(ChangeLevel::Patch)
        );
    }

    #[test]
    fn test_from_versions_minor() {
        assert_eq!(
            ChangeLevel::from_versions("1.2.3", "1.3.0"),
            Some(ChangeLevel::Minor)
        );
    }

    #[test]
    fn test_from_versions_major() {
        assert_eq!(
            ChangeLevel::from_versions("1.2.3", "2.0.0"),
            Some(ChangeLevel::Major)
        );
    }

    #[test]
    fn test_from_versions_equal_returns_none() {
        assert_eq!(ChangeLevel::from_versions("1.2.3", "1.2.3"), None);
    }

    #[test]
    fn test_from_versions_handles_v_prefix() {
        assert_eq!(
            ChangeLevel::from_versions("v1.2.3", "v1.2.4"),
            Some(ChangeLevel::Patch)
        );
        assert_eq!(
            ChangeLevel::from_versions("1.2.3", "v2.0.0"),
            Some(ChangeLevel::Major)
        );
    }

    #[test]
    fn test_from_versions_handles_prerelease_metadata() {
        // -rc.1 やビルドメタデータは無視
        assert_eq!(
            ChangeLevel::from_versions("1.2.3-rc.1", "1.2.4"),
            Some(ChangeLevel::Patch)
        );
        assert_eq!(
            ChangeLevel::from_versions("1.2.3+sha.abc", "1.3.0+sha.def"),
            Some(ChangeLevel::Minor)
        );
    }

    #[test]
    fn test_from_versions_unparseable_returns_none() {
        assert_eq!(ChangeLevel::from_versions("abc", "def"), None);
    }

    #[test]
    fn test_from_versions_short_versions() {
        // "1" だけのバージョンでも major は判定可能
        assert_eq!(
            ChangeLevel::from_versions("1", "2"),
            Some(ChangeLevel::Major)
        );
    }

    #[test]
    fn test_from_versions_four_part_truncated() {
        // 4 セグメント以上は先頭 3 つだけ見るので、4 つ目の違いは検出されない
        assert_eq!(
            ChangeLevel::from_versions("1.2.3.4", "1.2.3.5"),
            None,
            "本実装は先頭 3 セグメントしか比較しない"
        );
    }

    /// 回帰テスト: 欠落セグメントは 0 として比較する。
    /// 以前は `from_versions("1", "1.0.1")` が `None != Some(0)` 比較で
    /// Minor になっていた (正しくは Patch)。
    #[test]
    fn test_from_versions_missing_segments_compared_as_zero() {
        assert_eq!(
            ChangeLevel::from_versions("1", "1.0.1"),
            Some(ChangeLevel::Patch)
        );
        assert_eq!(
            ChangeLevel::from_versions("1.2", "1.3"),
            Some(ChangeLevel::Minor)
        );
        assert_eq!(
            ChangeLevel::from_versions("1", "2"),
            Some(ChangeLevel::Major)
        );
        // 0 補完で完全一致するなら None
        assert_eq!(ChangeLevel::from_versions("1", "1.0.0"), None);
        assert_eq!(ChangeLevel::from_versions("1.0", "1"), None);
        // 逆方向 (ダウングレード) でもレベル自体は同じ
        assert_eq!(
            ChangeLevel::from_versions("1.0.1", "1"),
            Some(ChangeLevel::Patch)
        );
    }

    /// 回帰テスト: 非数値セグメントを「読み飛ばして詰める」位置ずれ比較をしない。
    /// セグメントは先頭の数値プレフィックスのみを取り (例: "0rc1" → 0)、
    /// 数値が全く無いセグメント以降は無視する (`numeric_core` と共通規則)。
    #[test]
    fn test_from_versions_non_numeric_segments() {
        // "0rc1" は数値プレフィックス 0 として比較される
        assert_eq!(
            ChangeLevel::from_versions("1.0.0rc1", "1.0.1"),
            Some(ChangeLevel::Patch)
        );
        // qualifier (RELEASE / Final) 以降は無視される
        assert_eq!(
            ChangeLevel::from_versions("5.0.0.RELEASE", "5.0.1"),
            Some(ChangeLevel::Patch)
        );
        assert_eq!(ChangeLevel::from_versions("5.0.0.RELEASE", "5.0.0"), None);
        // 以前の filter_map 実装では "1.x.2" が [1, 2] に詰められ
        // minor 位置に 2 が来る位置ずれ比較になっていた
        assert_eq!(ChangeLevel::from_versions("1.x.2", "1.0.0"), None);
        // PEP 440 の dev セグメントも数値コアには影響しない
        assert_eq!(
            ChangeLevel::from_versions("1.0.1.dev1", "1.0.2"),
            Some(ChangeLevel::Patch)
        );
    }

    #[test]
    fn test_display_and_as_str() {
        assert_eq!(ChangeLevel::Patch.as_str(), "patch");
        assert_eq!(format!("{}", ChangeLevel::Minor), "minor");
        assert_eq!(ChangeLevel::Major.to_string(), "major");
    }
}
