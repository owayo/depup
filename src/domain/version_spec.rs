//! 各パッケージエコシステムで使うバージョン指定型。
//!
//! 例:
//! - Node.js: `^1.2.3`, `~1.2.3`, `>=1.0.0`, `1.2.3`
//! - Python: `^1.2.3`, `~1.2.3`, `>=1.2.3`, `==1.2.3`
//! - Rust: `1.2.3`, `^1.2.3`, `~1.2.3`, `=1.2.3`
//! - Go: `v1.2.3`, `// pinned`

use serde::{Deserialize, Serialize};
use std::fmt;

/// バージョン指定の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionSpecKind {
    /// 固定バージョン。例: Node の `1.2.3`、Python の `==1.2.3`、Rust の `=1.2.3`
    Exact,
    /// Caret レンジ。例: `^1.2.3`
    Caret,
    /// Tilde レンジ。例: `~1.2.3`
    Tilde,
    /// 以上。例: `>=1.2.3`
    GreaterOrEqual,
    /// より大きい。例: `>1.2.3`
    Greater,
    /// 以下。例: `<=1.2.3`
    LessOrEqual,
    /// より小さい。例: `<1.2.3`
    Less,
    /// ワイルドカード。例: `1.2.*`, `1.2.x`, `1.2.+`
    Wildcard,
    /// 複合レンジ。例: `>=1.0.0 <2.0.0`
    Range,
    /// `// pinned` コメント付き Go バージョン
    GoPinned,
    /// 制約なし。例: `gem 'rails'`
    Any,
}

impl VersionSpecKind {
    /// 固定バージョンとして扱う種類かどうかを返す
    pub fn is_pinned(&self) -> bool {
        matches!(self, VersionSpecKind::Exact | VersionSpecKind::GoPinned)
    }
}

/// 元の文字列表現も保持したバージョン指定
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionSpec {
    /// バージョン指定の種類
    pub kind: VersionSpecKind,
    /// マニフェスト上の元の文字列
    pub raw: String,
    /// 抽出したバージョン番号。prefix/suffix は含めない
    pub version: String,
    /// 更新時に保持する接頭辞。例: `^`, `~`, `>=`
    pub prefix: Option<String>,
    /// 更新時に保持する接尾辞。例: コメント
    pub suffix: Option<String>,
}

fn format_wildcard_like(raw: &str, new_version: &str) -> Option<String> {
    let trimmed = raw.trim();
    if matches!(trimmed, "*" | "x" | "X") {
        return Some(trimmed.to_string());
    }

    let numeric_head = new_version
        .strip_prefix('v')
        .or_else(|| new_version.strip_prefix('V'))
        .unwrap_or(new_version)
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .next()
        .unwrap_or("");

    if numeric_head.is_empty() {
        return Some(trimmed.to_string());
    }

    let mut parts: Vec<String> = numeric_head
        .split('.')
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect();

    if parts.is_empty() {
        return Some(trimmed.to_string());
    }

    let segments: Vec<&str> = trimmed.split('.').collect();
    while parts.len() < segments.len() {
        parts.push("0".to_string());
    }

    let mut rebuilt = Vec::with_capacity(segments.len());
    let mut has_numeric_anchor = false;

    for (index, segment) in segments.iter().enumerate() {
        let (prefix, core) = if index == 0 {
            if let Some(rest) = segment.strip_prefix('v') {
                ("v", rest)
            } else if let Some(rest) = segment.strip_prefix('V') {
                ("V", rest)
            } else {
                ("", *segment)
            }
        } else {
            ("", *segment)
        };

        let rebuilt_segment = if !core.is_empty() && core.chars().all(|ch| ch.is_ascii_digit()) {
            has_numeric_anchor = true;
            format!("{}{}", prefix, parts[index])
        } else if matches!(core, "*" | "x" | "X" | "+") {
            format!("{}{}", prefix, core)
        } else {
            return None;
        };

        rebuilt.push(rebuilt_segment);
    }

    if !has_numeric_anchor {
        return Some(trimmed.to_string());
    }

    Some(rebuilt.join("."))
}

impl VersionSpec {
    /// 新しい VersionSpec を作る
    pub fn new(kind: VersionSpecKind, raw: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            kind,
            raw: raw.into(),
            version: version.into(),
            prefix: None,
            suffix: None,
        }
    }

    /// 接頭辞付きの VersionSpec を作る
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// 接尾辞付きの VersionSpec を作る
    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    /// 既定では更新しない固定バージョンかどうかを返す
    pub fn is_pinned(&self) -> bool {
        self.kind.is_pinned()
    }

    /// 元の書式を保ちながら新しいバージョン文字列を組み立てる
    pub fn format_updated(&self, new_version: &str) -> String {
        if self.kind == VersionSpecKind::Wildcard
            && let Some(formatted) = format_wildcard_like(&self.raw, new_version)
        {
            return formatted;
        }

        let mut result = String::new();

        if let Some(ref prefix) = self.prefix {
            result.push_str(prefix);
        }

        result.push_str(new_version);

        if let Some(ref suffix) = self.suffix {
            result.push_str(suffix);
        }

        result
    }
}

impl fmt::Display for VersionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_spec_kind_is_pinned() {
        assert!(VersionSpecKind::Exact.is_pinned());
        assert!(VersionSpecKind::GoPinned.is_pinned());
        assert!(!VersionSpecKind::Caret.is_pinned());
        assert!(!VersionSpecKind::Tilde.is_pinned());
        assert!(!VersionSpecKind::GreaterOrEqual.is_pinned());
        assert!(!VersionSpecKind::Range.is_pinned());
        assert!(!VersionSpecKind::Any.is_pinned());
    }

    #[test]
    fn test_version_spec_kind_any() {
        let spec = VersionSpec::new(VersionSpecKind::Any, "", "");
        assert_eq!(spec.kind, VersionSpecKind::Any);
        assert!(!spec.is_pinned());
        // Any は新しい値をそのまま返す
        assert_eq!(spec.format_updated("1.2.3"), "1.2.3");
    }

    #[test]
    fn test_version_spec_new() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.raw, "^1.2.3");
        assert_eq!(spec.version, "1.2.3");
        assert!(spec.prefix.is_none());
        assert!(spec.suffix.is_none());
    }

    #[test]
    fn test_version_spec_with_prefix() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3").with_prefix("^");
        assert_eq!(spec.prefix, Some("^".to_string()));
    }

    #[test]
    fn test_version_spec_with_suffix() {
        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.2.3 // pinned", "1.2.3")
            .with_suffix(" // pinned");
        assert_eq!(spec.suffix, Some(" // pinned".to_string()));
    }

    #[test]
    fn test_version_spec_is_pinned() {
        let pinned = VersionSpec::new(VersionSpecKind::Exact, "1.2.3", "1.2.3");
        assert!(pinned.is_pinned());

        let not_pinned = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        assert!(!not_pinned.is_pinned());
    }

    #[test]
    fn test_format_updated_simple() {
        let spec = VersionSpec::new(VersionSpecKind::Exact, "1.2.3", "1.2.3");
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
    }

    #[test]
    fn test_format_updated_with_prefix() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3").with_prefix("^");
        assert_eq!(spec.format_updated("2.0.0"), "^2.0.0");
    }

    #[test]
    fn test_format_updated_with_prefix_and_suffix() {
        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.2.3 // pinned", "1.2.3")
            .with_prefix("v")
            .with_suffix(" // pinned");
        assert_eq!(spec.format_updated("2.0.0"), "v2.0.0 // pinned");
    }

    #[test]
    fn test_format_updated_tilde() {
        let spec = VersionSpec::new(VersionSpecKind::Tilde, "~1.2.3", "1.2.3").with_prefix("~");
        assert_eq!(spec.format_updated("1.3.0"), "~1.3.0");
    }

    #[test]
    fn test_format_updated_greater_or_equal() {
        let spec =
            VersionSpec::new(VersionSpecKind::GreaterOrEqual, ">=1.2.3", "1.2.3").with_prefix(">=");
        assert_eq!(spec.format_updated("2.0.0"), ">=2.0.0");
    }

    #[test]
    fn test_format_updated_wildcard_major() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.*", "1");
        assert_eq!(spec.format_updated("2.3.4"), "2.*");
    }

    #[test]
    fn test_format_updated_wildcard_minor_x() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.2.x", "1.2");
        assert_eq!(spec.format_updated("2.3.4"), "2.3.x");
    }

    #[test]
    fn test_format_updated_wildcard_multiple_positions() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.x.x", "1");
        assert_eq!(spec.format_updated("2.3.4"), "2.x.x");
    }

    #[test]
    fn test_format_updated_wildcard_gradle_prefix() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.+", "1");
        assert_eq!(spec.format_updated("2.3.4"), "2.+");
    }

    #[test]
    fn test_format_updated_wildcard_preserves_v_prefix() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "v1.*", "1");
        assert_eq!(spec.format_updated("2.3.4"), "v2.*");
    }

    #[test]
    fn test_format_updated_floating_wildcard_stays_same() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "*", "");
        assert_eq!(spec.format_updated("2.3.4"), "*");
    }

    #[test]
    fn test_format_updated_floating_multi_segment_wildcard_stays_same() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "x.x", "");
        assert_eq!(spec.format_updated("2.3.4"), "x.x");
    }

    #[test]
    fn test_display_trait() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        assert_eq!(format!("{}", spec), "^1.2.3");
    }

    #[test]
    fn test_version_spec_equality() {
        let spec1 = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        let spec2 = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        assert_eq!(spec1, spec2);
    }

    #[test]
    fn test_version_spec_clone() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        let cloned = spec.clone();
        assert_eq!(spec, cloned);
    }

    #[test]
    fn test_serde_version_spec_kind() {
        let kind = VersionSpecKind::GreaterOrEqual;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"greater_or_equal\"");

        let parsed: VersionSpecKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }

    #[test]
    fn test_serde_version_spec() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3").with_prefix("^");
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: VersionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, spec);
    }
}
