//! Cargo の semver 互換の系列判定。
//!
//! Cargo の resolver は同じ source の同名 crate について、互換の系列ごとに 1 版しか
//! lock に置けない (`1.2.0` と `1.3.0` は共存できないが、`0.2.0` と `0.3.0` は共存できる)。
//! 系列の切り方は Cargo 本体の `SemverCompatibility` と同じ。

/// Cargo が 1 つの lock に 1 版しか置けない semver 互換の系列。
/// `1.2.3` → `Major(1)`、`0.2.3` → `Minor(2)`、`0.0.3` → `Patch(3)`。prerelease / build metadata は系列に影響しない
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemverSeries {
    Major(u64),
    Minor(u64),
    Patch(u64),
}

/// `semver::Version` として読めなければ None
pub fn semver_series(version: &str) -> Option<SemverSeries> {
    let version = semver::Version::parse(version).ok()?;
    Some(if version.major != 0 {
        SemverSeries::Major(version.major)
    } else if version.minor != 0 {
        SemverSeries::Minor(version.minor)
    } else {
        SemverSeries::Patch(version.patch)
    })
}

/// 両方が読めて同じ系列なら true
pub fn same_series(a: &str, b: &str) -> bool {
    match (semver_series(a), semver_series(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_series_major() {
        assert_eq!(semver_series("1.2.3"), Some(SemverSeries::Major(1)));
        assert_eq!(semver_series("2.0.0"), Some(SemverSeries::Major(2)));
        // major が 0 でなければ minor / patch が 0 でも major で切る
        assert_eq!(semver_series("10.0.0"), Some(SemverSeries::Major(10)));
    }

    #[test]
    fn test_semver_series_minor() {
        assert_eq!(semver_series("0.2.3"), Some(SemverSeries::Minor(2)));
        assert_eq!(semver_series("0.10.0"), Some(SemverSeries::Minor(10)));
    }

    #[test]
    fn test_semver_series_patch() {
        assert_eq!(semver_series("0.0.3"), Some(SemverSeries::Patch(3)));
        assert_eq!(semver_series("0.0.0"), Some(SemverSeries::Patch(0)));
    }

    #[test]
    fn test_semver_series_ignores_prerelease_and_build_metadata() {
        assert_eq!(semver_series("1.0.0-alpha.1"), Some(SemverSeries::Major(1)));
        assert_eq!(semver_series("0.3.0-rc.2"), Some(SemverSeries::Minor(3)));
        assert_eq!(
            semver_series("0.11.0+wasi-snapshot-preview1"),
            Some(SemverSeries::Minor(11))
        );
        assert_eq!(
            semver_series("0.0.7-beta.1+build.5"),
            Some(SemverSeries::Patch(7))
        );
    }

    #[test]
    fn test_semver_series_unreadable() {
        for version in [
            "", "1.2", "1", "1.2.3.4", "v1.2.3", "01.2.3", "abc", " 1.2.3",
        ] {
            assert_eq!(semver_series(version), None, "version: {version:?}");
        }
    }

    #[test]
    fn test_same_series() {
        // 1.x は major が同じなら同じ系列
        assert!(same_series("1.2.3", "1.9.0"));
        assert!(!same_series("1.2.3", "2.0.0"));
        // 0.x は minor で切る
        assert!(same_series("0.2.3", "0.2.9"));
        assert!(!same_series("0.2.3", "0.3.0"));
        // 0.0.x は patch ごとに別系列
        assert!(same_series("0.0.3", "0.0.3"));
        assert!(!same_series("0.0.3", "0.0.4"));
        // 数字が同じでも桁の位置が違えば別系列
        assert!(!same_series("1.0.0", "0.1.0"));
        assert!(!same_series("0.1.0", "0.0.1"));
        // prerelease / build metadata は無視する
        assert!(same_series("1.0.0-rc.1", "1.2.0"));
        assert!(same_series(
            "0.11.0+wasi-snapshot-preview1",
            "0.11.1+wasi-0.2.4"
        ));
    }

    #[test]
    fn test_same_series_requires_both_readable() {
        assert!(!same_series("1.2.3", "1.2"));
        assert!(!same_series("v1.2.3", "1.2.3"));
        // 同じ文字列でも読めなければ false
        assert!(!same_series("abc", "abc"));
    }
}
