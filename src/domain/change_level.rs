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
    /// プレリリース・ビルドメタデータは無視)。両方ともパースできない場合や、
    /// 数値部分が完全一致する場合は `None`。
    pub fn from_versions(old: &str, new: &str) -> Option<Self> {
        let old_p = split_core(old);
        let new_p = split_core(new);
        if old_p.is_empty() || new_p.is_empty() {
            return None;
        }
        if old_p.first() != new_p.first() {
            Some(ChangeLevel::Major)
        } else if old_p.get(1) != new_p.get(1) {
            Some(ChangeLevel::Minor)
        } else if old_p.get(2) != new_p.get(2) {
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

/// `v` プレフィックスとプレリリース/ビルドメタデータを取り除き、`.` 区切りの
/// 先頭 3 セグメントを数値としてパースする。
fn split_core(s: &str) -> Vec<u64> {
    let trimmed = s.trim().trim_start_matches(['v', 'V']);
    let core = trimmed.split(['-', '+']).next().unwrap_or("");
    core.split('.')
        .filter_map(|p| p.parse::<u64>().ok())
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

    #[test]
    fn test_display_and_as_str() {
        assert_eq!(ChangeLevel::Patch.as_str(), "patch");
        assert_eq!(format!("{}", ChangeLevel::Minor), "minor");
        assert_eq!(ChangeLevel::Major.to_string(), "major");
    }
}
