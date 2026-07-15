//! 各パッケージエコシステムで使うバージョン指定型。
//!
//! 例:
//! - Node.js の例: `^1.2.3`, `~1.2.3`, `>=1.0.0`, `1.2.3`
//! - Python の例: `^1.2.3`, `~1.2.3`, `>=1.2.3`, `==1.2.3`
//! - Rust の例: `1.2.3`, `^1.2.3`, `~1.2.3`, `=1.2.3`
//! - Go の例: `v1.2.3`, `// pinned`

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
    /// 更新候補から除外するバージョン。例: Gradle rich version の `reject`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_versions: Vec<String>,
}

fn extract_numeric_parts(new_version: &str) -> Option<Vec<String>> {
    let numeric_head = new_version
        .strip_prefix('v')
        .or_else(|| new_version.strip_prefix('V'))
        .unwrap_or(new_version)
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .next()
        .unwrap_or("");

    let parts: Vec<String> = numeric_head
        .split('.')
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect();

    if parts.is_empty() { None } else { Some(parts) }
}

fn format_wildcard_like(raw: &str, new_version: &str) -> Option<String> {
    let trimmed = raw.trim();
    // npm の `^1.x` / `~1.2.*` や Cargo の `=1.*` / `^1.*` のような演算子付きワイルドカードでは、
    // 先頭の `^` / `~` / `=` 演算子を切り出して保持し、残りをワイルドカードとして再構成する。
    // 既存のワイルドカード (`1.x` / `1.2.*` / `v1.*` / `1.+`) は演算子を持たないため
    // op_prefix は空となり、従来どおりの挙動になる。
    let op_len = trimmed
        .bytes()
        .take_while(|b| matches!(b, b'^' | b'~' | b'='))
        .count();
    let op_prefix = &trimmed[..op_len];
    let body = trimmed[op_len..].trim_start();

    if matches!(body, "*" | "x" | "X") {
        return Some(format!("{op_prefix}{body}"));
    }

    let Some(mut parts) = extract_numeric_parts(new_version) else {
        return Some(trimmed.to_string());
    };

    let segments: Vec<&str> = body.split('.').collect();
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

    Some(format!("{op_prefix}{}", rebuilt.join(".")))
}

fn format_partial_version_like(raw: &str, new_version: &str) -> Option<String> {
    let trimmed = raw.trim();
    let (op_prefix, body) = if let Some(rest) = trimmed.strip_prefix('=') {
        ("=", rest.trim_start())
    } else {
        ("", trimmed)
    };
    let (version_prefix, core) = if let Some(rest) = body.strip_prefix('v') {
        ("v", rest)
    } else if let Some(rest) = body.strip_prefix('V') {
        ("V", rest)
    } else {
        ("", body)
    };

    if core.is_empty()
        || !core
            .split('.')
            .all(|segment| !segment.is_empty() && segment.chars().all(|ch| ch.is_ascii_digit()))
    {
        return None;
    }

    let segment_count = core.split('.').count();
    let mut parts = extract_numeric_parts(new_version)?;
    while parts.len() < segment_count {
        parts.push("0".to_string());
    }

    Some(format!(
        "{}{}{}",
        op_prefix,
        version_prefix,
        parts[..segment_count].join(".")
    ))
}

fn preserve_version_prefix(template: &str, new_version: &str) -> String {
    let stripped = new_version
        .strip_prefix('v')
        .or_else(|| new_version.strip_prefix('V'))
        .unwrap_or(new_version);

    if template.starts_with('V') {
        format!("V{}", stripped)
    } else if template.starts_with('v') {
        format!("v{}", stripped)
    } else {
        stripped.to_string()
    }
}

fn find_first_version_token(raw: &str) -> Option<(usize, usize)> {
    let chars: Vec<(usize, char)> = raw.char_indices().collect();

    for (index, &(start, ch)) in chars.iter().enumerate() {
        let looks_like_start = ch.is_ascii_digit()
            || ((ch == 'v' || ch == 'V')
                && chars
                    .get(index + 1)
                    .map(|(_, next)| next.is_ascii_digit())
                    .unwrap_or(false));
        if !looks_like_start {
            continue;
        }

        let mut end = raw.len();
        for (scan_idx, &(candidate_end, candidate)) in chars.iter().enumerate().skip(index + 1) {
            // 連続するドット (..) はレンジ演算子 (..< / ...) なのでトークンを終端する
            if candidate == '.'
                && chars
                    .get(scan_idx + 1)
                    .map(|(_, next)| *next == '.')
                    .unwrap_or(false)
            {
                end = candidate_end;
                break;
            }
            if !(candidate.is_ascii_alphanumeric()
                || matches!(candidate, '.' | '*' | '+' | '-' | '_'))
            {
                end = candidate_end;
                break;
            }
        }

        return Some((start, end));
    }

    None
}

fn replace_version_token(raw: &str, start: usize, end: usize, new_version: &str) -> Option<String> {
    let token = &raw[start..end];
    let replacement = if token.contains('*') {
        format_wildcard_like(token, new_version)?
    } else {
        preserve_version_prefix(token, new_version)
    };

    Some(format!("{}{}{}", &raw[..start], replacement, &raw[end..]))
}

fn replace_version_token_preserving_shape(
    raw: &str,
    start: usize,
    end: usize,
    new_version: &str,
) -> Option<String> {
    let token = &raw[start..end];
    let replacement = if token.contains(['x', 'X', '*']) {
        format_wildcard_like(token, new_version)?
    } else {
        format_partial_version_like(token, new_version)?
    };

    Some(format!("{}{}{}", &raw[..start], replacement, &raw[end..]))
}

fn find_version_token_at(raw: &str, offset: usize) -> Option<(usize, usize)> {
    let rest = raw.get(offset..)?;
    let whitespace_len = rest.len() - rest.trim_start().len();
    let token_start = offset + whitespace_len;
    let token_rest = raw.get(token_start..)?;
    let (start, end) = find_first_version_token(token_rest)?;
    if start == 0 {
        Some((token_start, token_start + end))
    } else {
        None
    }
}

fn find_gradle_strict_prefer_token(raw: &str) -> Option<(usize, usize)> {
    let bang_index = raw.find("!!")?;
    let strict_part = raw[..bang_index].trim();
    if !matches!(strict_part.chars().next(), Some('[' | '(' | ']')) {
        return None;
    }

    find_version_token_at(raw, bang_index + 2)
}

fn find_inclusive_lower_bound_token(raw: &str) -> Option<(usize, usize)> {
    let operators = [">=", "~=", "==", "=", "^", "~"];
    let mut index = 0;

    while index < raw.len() {
        let rest = &raw[index..];
        let is_operator_continuation =
            index > 0 && matches!(raw.as_bytes()[index - 1], b'<' | b'>' | b'!' | b'=' | b'~');
        if is_operator_continuation {
            let ch = rest.chars().next()?;
            index += ch.len_utf8();
            continue;
        }

        for operator in operators {
            if rest.starts_with(operator) {
                let after_operator = index + operator.len();
                if let Some(token) = find_version_token_at(raw, after_operator) {
                    return Some(token);
                }
            }
        }

        let ch = rest.chars().next()?;
        index += ch.len_utf8();
    }

    None
}

fn find_bare_lower_bound_token(raw: &str) -> Option<(usize, usize)> {
    let leading_ws_len = raw.len() - raw.trim_start().len();
    find_version_token_at(raw, leading_ws_len)
}

/// レンジ文字列から比較基準にする包含下限のバージョン文字列を返す。
///
/// `>=` / `~=` / `==` / `=` / `^` / `~` の直後、または裸の下限トークンを、
/// カンマ/空白区切りの記述順に依存せず探して返す。書き換え側 (`format_range_like`)
/// と同じトークン探索を使うため、judge が使う比較基準 version と、実際に writer が
/// 書き換えるトークンが必ず一致する。これにより上限を先に書いたレンジ
/// (`<1.5, >=1.2.2` など) でも下限 `1.2.2` を基準にでき、AlreadyLatest 誤判定による
/// 更新取りこぼしを防ぐ。包含下限が無い場合 (厳密下限 `>1.0` のみ等) は `None` を返し、
/// 呼び出し側の従来ロジックにフォールバックさせる。
pub fn range_lower_bound_version(raw: &str) -> Option<String> {
    let (start, end) =
        find_inclusive_lower_bound_token(raw).or_else(|| find_bare_lower_bound_token(raw))?;
    Some(raw[start..end].to_string())
}

fn contains_not_equal_operator(raw: &str) -> bool {
    // `!==` は各エコシステムの有効な制約ではないが、`!=` を含むので同じく拒否する。
    // Composer (composer/semver) は not-equal を `!=` と `<>` の両方で綴れる
    // (演算子パターン `(<>|!=|>=?|<=?|==?)`)。`<>` を含む制約も除外制約なので、
    // `!=` と同様に安全側でスキップする (下限だけ書き換えると除外バージョンを
    // 選んで充足不能な制約 `>=1.5.0 <>1.5.0 <2.0` を生む恐れがあるため)。
    raw.as_bytes()
        .windows(2)
        .any(|window| window == b"!=" || window == b"<>")
}

fn format_range_like(raw: &str, new_version: &str) -> Option<String> {
    let trimmed = raw.trim();
    let leading_ws_len = raw.len() - raw.trim_start().len();

    if let Some((start, end)) = find_gradle_strict_prefer_token(raw) {
        return replace_version_token(raw, start, end, new_version);
    }

    if trimmed.contains("||") || contains_not_equal_operator(trimmed) || trimmed.starts_with("===")
    {
        return None;
    }

    // 単一制約の `~=1.2.3` のみ、演算子を保持しつつセグメント数を維持して下限を進める。
    // セグメント数を変えると上限の意味が変わる (`~=1.2` の上限 <2.0 が `~=1.9.0` だと
    // <1.10.0 になる) ため、format_partial_version_like でセグメント数を保つ。
    // `~=1.2, <1.5` のような複合制約は横取りせず、下の find_inclusive_lower_bound_token
    // 経路に任せる (横取りすると body にカンマ以降が混ざり format に失敗する)。
    if !trimmed.contains(',')
        && let Some(rest) = trimmed.strip_prefix("~=")
    {
        let spacing_len = rest.len() - rest.trim_start().len();
        let spacing = &rest[..spacing_len];
        let body = rest.trim();
        return format_partial_version_like(body, new_version)
            .map(|formatted| format!("~={spacing}{formatted}"));
    }

    if let Some(rest) = trimmed.strip_prefix("==") {
        let spacing_len = rest.len() - rest.trim_start().len();
        let spacing = &rest[..spacing_len];
        let body = rest.trim();
        if body.contains('*') {
            return format_wildcard_like(body, new_version)
                .map(|formatted| format!("=={}{}", spacing, formatted));
        }
    }

    let has_explicit_range_syntax = trimmed.contains('<')
        || trimmed.contains('>')
        || trimmed.contains(',')
        || trimmed.contains(" - ")
        || trimmed.contains("..<")
        || trimmed.contains("...");

    if !has_explicit_range_syntax
        && !trimmed.starts_with('[')
        && !trimmed.starts_with('(')
        && !trimmed.starts_with(']')
    {
        return format_partial_version_like(trimmed, new_version);
    }

    if matches!(trimmed.chars().next(), Some('[' | '(' | ']')) {
        let comma_index = trimmed.find(',')?;
        let lower = trimmed[1..comma_index].trim();
        if lower.is_empty() || !trimmed.starts_with('[') {
            return None;
        }

        let lower_offset = leading_ws_len + 1;
        let lower_start = find_version_token_at(raw, lower_offset)?;
        if lower_start.0 >= leading_ws_len + comma_index {
            return None;
        }

        return replace_version_token(raw, lower_start.0, lower_start.1, new_version);
    }

    if trimmed.contains(" - ") || trimmed.contains("..<") || trimmed.contains("...") {
        let (start, end) = find_first_version_token(raw)?;
        return replace_version_token(raw, start, end, new_version);
    }

    // カンマ区切りの複数要件で上限 (`<` / `<=`) がない場合 (例: `>=1.2.3, ^1.3`)、
    // 包含下限だけを進めると充足不能なレンジになり得るため安全に書き換えられない。
    // (単一の包含下限 `>=1.0` は上限がなくても最新へ進められるので対象外)
    if trimmed.contains(',') && !trimmed.contains('<') {
        return None;
    }

    if let Some((start, end)) = find_inclusive_lower_bound_token(raw) {
        return replace_version_token(raw, start, end, new_version);
    }

    let (start, end) = find_bare_lower_bound_token(raw)?;

    replace_version_token_preserving_shape(raw, start, end, new_version)
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
            rejected_versions: Vec::new(),
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

    /// 拒否バージョン一覧付きの VersionSpec を作る
    pub fn with_rejected_versions<I, S>(mut self, versions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.rejected_versions = versions.into_iter().map(Into::into).collect();
        self
    }

    /// 既定では更新しない固定バージョンかどうかを返す
    pub fn is_pinned(&self) -> bool {
        self.kind.is_pinned()
    }

    /// 安全に更新後の文字列表現を組み立てられる場合だけ返す
    pub fn try_format_updated(&self, new_version: &str) -> Option<String> {
        match self.kind {
            VersionSpecKind::Wildcard => format_wildcard_like(&self.raw, new_version),
            VersionSpecKind::Range => format_range_like(&self.raw, new_version),
            VersionSpecKind::Greater | VersionSpecKind::LessOrEqual | VersionSpecKind::Less => None,
            _ => {
                let mut result = String::new();

                if let Some(ref prefix) = self.prefix {
                    result.push_str(prefix);
                }

                result.push_str(new_version);

                if let Some(ref suffix) = self.suffix {
                    result.push_str(suffix);
                }

                Some(result)
            }
        }
    }

    /// 元の書式を保ちながら新しいバージョン文字列を組み立てる
    pub fn format_updated(&self, new_version: &str) -> String {
        self.try_format_updated(new_version)
            .unwrap_or_else(|| self.raw.clone())
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
    fn test_try_format_updated_rejects_strict_greater() {
        // `>最新候補` に書き換えると、その最新候補自身が制約を満たさない
        let spec = VersionSpec::new(VersionSpecKind::Greater, ">1.2.3", "1.2.3").with_prefix(">");
        assert!(spec.try_format_updated("2.0.0").is_none());
    }

    #[test]
    fn test_try_format_updated_rejects_upper_bound_only_constraints() {
        // 上限だけの制約を書き換えると許容範囲を広げるため安全ではない
        let less = VersionSpec::new(VersionSpecKind::Less, "<2.0.0", "2.0.0").with_prefix("<");
        let less_or_equal =
            VersionSpec::new(VersionSpecKind::LessOrEqual, "<=2.0.0", "2.0.0").with_prefix("<=");

        assert!(less.try_format_updated("3.0.0").is_none());
        assert!(less_or_equal.try_format_updated("3.0.0").is_none());
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
    fn test_try_format_updated_range_replaces_lower_bound() {
        let spec = VersionSpec::new(VersionSpecKind::Range, ">=1.0,<2.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some(">=1.9.3,<2.0")
        );
    }

    #[test]
    fn test_try_format_updated_range_replaces_bare_lower_bound() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.2 <2.0.0", "1.2.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some("1.9 <2.0.0")
        );
    }

    #[test]
    fn test_try_format_updated_range_preserves_spacing() {
        let spec = VersionSpec::new(VersionSpecKind::Range, ">= 1.0, < 2.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some(">= 1.9.3, < 2.0")
        );
    }

    #[test]
    fn test_try_format_updated_range_updates_inclusive_lower_bound_when_ordered_later() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "<=2.0,>=1.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some("<=2.0,>=1.9.3")
        );
    }

    #[test]
    fn test_try_format_updated_range_rejects_exclusive_lower_bound() {
        // `>最新候補` に書き換えると、その最新候補自身が制約を満たさない
        let spec = VersionSpec::new(VersionSpecKind::Range, ">1.0,<2.0", "1.0");
        assert!(spec.try_format_updated("1.9.3").is_none());
    }

    #[test]
    fn test_try_format_updated_range_hyphen_updates_left_side() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.0 - 2.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some("1.9.3 - 2.0")
        );
    }

    #[test]
    fn test_try_format_updated_range_maven_updates_lower_bound() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "[1.0,2.0)", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some("[1.9.3,2.0)")
        );
    }

    #[test]
    fn test_try_format_updated_range_maven_open_upper_updates_lower_bound() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "[1.0,)", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some("[1.9.3,)")
        );
    }

    #[test]
    fn test_try_format_updated_range_partial_version_preserves_shape() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.2", "1.2.0");
        assert_eq!(spec.try_format_updated("2.3.4").as_deref(), Some("2.3"));
    }

    #[test]
    fn test_try_format_updated_range_equal_partial_preserves_shape() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "=1.2", "1.2.0");
        assert_eq!(spec.try_format_updated("2.3.4").as_deref(), Some("=2.3"));
    }

    #[test]
    fn test_try_format_updated_range_python_prefix_wildcard() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "==1.2.*", "1.2");
        assert_eq!(spec.try_format_updated("2.3.4").as_deref(), Some("==2.3.*"));
    }

    #[test]
    fn test_try_format_updated_range_rejects_not_equal() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "!=1.2.3", "1.2.3");
        assert!(spec.try_format_updated("2.0.0").is_none());
    }

    #[test]
    fn test_try_format_updated_range_rejects_or_constraint() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "^1 || ^2", "1");
        assert!(spec.try_format_updated("2.0.0").is_none());
    }

    #[test]
    fn test_try_format_updated_range_maven_lower_open_returns_none() {
        // Maven 下限なし `(,2.0]` は安全に書き換えられないため None を返す
        let spec = VersionSpec::new(VersionSpecKind::Range, "(,2.0]", "0.0");
        assert!(spec.try_format_updated("1.5.0").is_none());
    }

    #[test]
    fn test_try_format_updated_range_maven_lower_open_exclusive_returns_none() {
        // Maven 下限なし `(,2.0)` も同様に None を返す
        let spec = VersionSpec::new(VersionSpecKind::Range, "(,2.0)", "0.0");
        assert!(spec.try_format_updated("1.5.0").is_none());
    }

    #[test]
    fn test_try_format_updated_range_arbitrary_equality_returns_none() {
        // `===` 付きレンジは安全に書き換えられない
        let spec = VersionSpec::new(VersionSpecKind::Range, "===v1.2-custom", "1.2");
        assert!(spec.try_format_updated("2.0.0").is_none());
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
    fn test_format_wildcard_like_v_prefix_upper() {
        // 大文字 V プレフィックス付きワイルドカードの更新
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "V1.*", "1");
        assert_eq!(spec.format_updated("2.3.4"), "V2.*");
    }

    #[test]
    fn test_format_range_like_maven_alt_brackets() {
        // Maven の反転ブラケット記法 ]...[ は下限排他なので安全に書き換えられない
        let spec = VersionSpec::new(VersionSpecKind::Range, "]1.0,2.0[", "1.0");
        assert!(spec.try_format_updated("1.5.0").is_none());
    }

    #[test]
    fn test_format_range_like_swift_half_open() {
        // Swift の半開区間 ..< は下限のみ更新し、上限と演算子を保持する
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.0.0..<2.0.0", "1.0.0");
        assert_eq!(
            spec.try_format_updated("1.5.0").as_deref(),
            Some("1.5.0..<2.0.0")
        );
    }

    #[test]
    fn test_format_range_like_swift_closed() {
        // Swift の閉区間 ... は下限のみ更新し、上限と演算子を保持する
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.0.0...2.0.0", "1.0.0");
        assert_eq!(
            spec.try_format_updated("1.5.0").as_deref(),
            Some("1.5.0...2.0.0")
        );
    }

    #[test]
    fn test_format_range_like_comma_not_equal_rejected() {
        // カンマ区切りの不等号制約は安全に書き換えられないため None を返す
        let spec = VersionSpec::new(VersionSpecKind::Range, ",!=1.2.3", "1.2.3");
        assert!(spec.try_format_updated("2.0.0").is_none());
    }

    #[test]
    fn test_format_range_like_spaced_not_equal_rejected() {
        // PEP 440 / Composer は `, !=` や空白区切りの `!=` を許容するが、
        // 除外候補を選ばない保証がないため自動更新では拒否する
        let comma_spaced =
            VersionSpec::new(VersionSpecKind::Range, ">= 1.0, != 1.5.0, < 2.0", "1.0");
        let space_separated = VersionSpec::new(VersionSpecKind::Range, ">=1.0 !=1.5.0 <2.0", "1.0");

        assert!(comma_spaced.try_format_updated("1.9.0").is_none());
        assert!(space_separated.try_format_updated("1.9.0").is_none());
    }

    #[test]
    fn test_format_range_like_shell_not_equal_rejected() {
        // Composer は not-equal を `<>` でも綴れる。`!=` と同様に、除外制約を含む
        // レンジは安全に書き換えられないため None を返す (下限だけ進めると除外
        // バージョンを選んで充足不能な制約を書き戻す恐れがある)。
        let comma = VersionSpec::new(VersionSpecKind::Range, ">=1.0,<>1.5.0,<2.0", "1.0");
        let spaced = VersionSpec::new(VersionSpecKind::Range, ">=1.0 <>1.5.0 <2.0", "1.0");

        assert!(comma.try_format_updated("1.9.0").is_none());
        assert!(spaced.try_format_updated("1.9.0").is_none());
    }

    #[test]
    fn test_try_format_updated_any_empty_prefix_suffix() {
        // Any 種別で prefix/suffix が空の場合、新バージョンをそのまま返す
        let spec = VersionSpec {
            kind: VersionSpecKind::Any,
            raw: String::new(),
            version: String::new(),
            prefix: None,
            suffix: None,
            rejected_versions: Vec::new(),
        };
        assert_eq!(spec.try_format_updated("1.2.3").as_deref(), Some("1.2.3"));
    }

    #[test]
    fn test_format_range_like_v_prefix_in_range() {
        // レンジ内の v プレフィックスが保持されて下限のみ更新される
        let spec = VersionSpec::new(VersionSpecKind::Range, ">=v1.0,<v2.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.5.0").as_deref(),
            Some(">=v1.5.0,<v2.0")
        );
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

    #[test]
    fn test_serde_version_spec_rejected_versions() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "[1.0,2.0[", "1.5")
            .with_rejected_versions(["1.6", "1.7"]);
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: VersionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.rejected_versions, vec!["1.6", "1.7"]);
    }

    // --- Swift レンジ演算子の追加テスト ---

    #[test]
    fn test_format_range_like_swift_half_open_two_segment() {
        // 2セグメントバージョンの半開区間
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.0..<2.0", "1.0");
        assert_eq!(spec.try_format_updated("1.5").as_deref(), Some("1.5..<2.0"));
    }

    #[test]
    fn test_format_range_like_swift_closed_different_major() {
        // メジャーバージョンが異なる閉区間
        let spec = VersionSpec::new(VersionSpecKind::Range, "2.0.0...3.0.0", "2.0.0");
        assert_eq!(
            spec.try_format_updated("2.5.0").as_deref(),
            Some("2.5.0...3.0.0")
        );
    }

    #[test]
    fn test_format_range_like_ruby_compound_comma() {
        // Ruby スタイルのカンマ区切り複合制約
        let spec = VersionSpec::new(VersionSpecKind::Range, ">= 1.0, < 2.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.8.0").as_deref(),
            Some(">= 1.8.0, < 2.0")
        );
    }

    #[test]
    fn test_format_range_like_maven_closed_brackets() {
        // Maven 閉区間 [A,B] の下限のみ更新
        let spec = VersionSpec::new(VersionSpecKind::Range, "[1.0,2.0]", "1.0");
        assert_eq!(
            spec.try_format_updated("1.5.0").as_deref(),
            Some("[1.5.0,2.0]")
        );
    }

    #[test]
    fn test_format_range_like_hyphen_range_preserves_spacing() {
        // ハイフンレンジのスペーシングが保持される
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.0.0 - 3.0.0", "1.0.0");
        assert_eq!(
            spec.try_format_updated("2.0.0").as_deref(),
            Some("2.0.0 - 3.0.0")
        );
    }

    #[test]
    fn test_format_wildcard_like_gradle_two_segment() {
        // Gradle の 2セグメント + ワイルドカード
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "5.3.+", "5.3");
        assert_eq!(spec.format_updated("6.1.0"), "6.1.+");
    }

    #[test]
    fn test_format_wildcard_like_caret_prefix() {
        // npm の caret + x-range は `^` を保持して形を保って更新する
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "^1.x", "1");
        assert_eq!(spec.format_updated("2.3.4"), "^2.x");
    }

    #[test]
    fn test_format_wildcard_like_tilde_prefix_minor() {
        // npm の tilde + x-range は `~` を保持する
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "~1.2.x", "1.2");
        assert_eq!(spec.format_updated("2.3.4"), "~2.3.x");
    }

    #[test]
    fn test_format_wildcard_like_no_operator_unchanged() {
        // 演算子なしの既存ワイルドカードは従来どおりの挙動 (op_prefix が空)
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.x", "1");
        assert_eq!(spec.format_updated("2.3.4"), "2.x");
    }

    #[test]
    fn test_format_range_like_pep440_compatible_release() {
        // PEP 440 の `~=` はセグメント数を保って下限を進める。
        // セグメント数を変えると上限の意味が変わる (`~=1.2` の <2.0 が `~=1.9.0` だと <1.10.0)。
        let three = VersionSpec::new(VersionSpecKind::Range, "~=1.2.3", "1.2.3");
        assert_eq!(three.format_updated("1.2.9"), "~=1.2.9");
        let two = VersionSpec::new(VersionSpecKind::Range, "~=1.2", "1.2");
        assert_eq!(two.format_updated("1.9.5"), "~=1.9");
    }

    #[test]
    fn test_format_range_like_pep440_compatible_release_compound() {
        // `~=1.2, <1.5` のような複合制約は ~= 分岐に横取りされず、下限側のみ進める (回帰防止)
        let spec = VersionSpec::new(VersionSpecKind::Range, "~=1.2, <1.5", "1.2");
        assert_eq!(spec.format_updated("1.4"), "~=1.4, <1.5");
    }
}
