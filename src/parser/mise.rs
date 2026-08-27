//! mise (jdx/mise) のツールバージョン指定パーサ
//!
//! mise は `mise.toml` の `[tools]` や `.tool-versions` でツールのバージョンを指定する。
//! 指定できる形式は次のとおり (公式ドキュメント "Configuration" の version specifier):
//!
//! | 記法 | 意味 | depup での扱い |
//! |---|---|---|
//! | `26.7.0` | 完全一致 | `Exact` として最新版へ更新 |
//! | `26` / `26.7` | 前方一致の最新 | `Prefix` としてセグメント数を保って更新 |
//! | `prefix:26` | 明示的な前方一致 | `Prefix`。`prefix:` を保持して更新 |
//! | `temurin-21.0.5` | ベンダー付きバージョン | ベンダーを接頭辞として保持し数値部を更新 |
//! | `latest` / `lts` | 常に最新 / LTS | 意味が変わらないので更新対象外 |
//! | `ref:master` | VCS ref からビルド | バージョンではないので更新対象外 |
//! | `path:/opt/node` | ローカルパス | 同上 |
//! | `sub-2:lts` | 解決値からの減算 | 浮動指定なので更新対象外 |
//! | `system` | システム標準 | 同上 |
//!
//! ベンダー接頭辞 (`temurin-` / `graalvm-community-` / `truffleruby+graalvm-` など) は
//! java / python / ruby で常用される。`mise ls-remote java` は 3000 件超のうち大半が
//! ベンダー付きで、接頭辞を無視して「最新版」を選ぶと `temurin-21` の利用者が
//! `zulu-27` へ飛ばされる。接頭辞は必ず保持し、候補も同じ接頭辞のものだけに絞る
//! (絞り込みは `crate::update` 側の `mise_flavor_candidates`)。

use super::VersionParser;
use crate::domain::{Language, VersionSpec, VersionSpecKind};

/// mise の「前方一致セレクタ」接頭辞
const PREFIX_SELECTOR: &str = "prefix:";

/// バージョンではなく解決方法を指定するセレクタ (更新対象外)
const NON_VERSION_SELECTORS: &[&str] = &["ref:", "path:", "sub-"];

/// 常に浮動する別名 (更新対象外)
const FLOATING_ALIASES: &[&str] = &["latest", "lts", "system"];

/// `Exact` と `Prefix` を分ける数値セグメント数の境界。
///
/// mise では `node = "26.7"` は「26.7 系の最新」を都度解決する前方一致指定で、
/// `node = "26.7.0"` が完全一致。多くのツール (node / python / go / terraform) が
/// `major.minor.patch` の 3 セグメントなので、3 未満を前方一致とみなす。
const EXACT_SEGMENT_COUNT: usize = 3;

/// mise のバージョン指定パーサ
pub struct MiseVersionParser;

/// ベンダー接頭辞とバージョン数値部を分割する。
///
/// `-` の直後が ASCII 数字になる最初の位置で切る。最後の `-` で切ると
/// `temurin-21.0.5-b1` が `temurin-21.0.5-` / `b1` に割れてしまうため、
/// 「最初に数値が始まるところ」を境界にする。
///
/// - `temurin-21.0.5` → `("temurin-", "21.0.5")`
/// - `graalvm-community-17.0.7` → `("graalvm-community-", "17.0.7")`
/// - `truffleruby+graalvm-24.1.1` → `("truffleruby+graalvm-", "24.1.1")`
/// - `26.7.0` → `("", "26.7.0")` (接頭辞なし)
/// - `jruby-dev` → `None` (数値部が無く、更新できない)
pub(crate) fn split_mise_flavor(value: &str) -> Option<(&str, &str)> {
    if value.is_empty() {
        return None;
    }
    if value.starts_with(|c: char| c.is_ascii_digit()) {
        return Some(("", value));
    }

    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'-'
            && bytes
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_digit())
        {
            return Some((&value[..=index], &value[index + 1..]));
        }
    }
    None
}

/// マニフェスト上の元の表記を手掛かりに、新しいバージョンの書き戻し表記を作る。
///
/// judge が返す新バージョンはベンダー接頭辞を落とした数値部 (`21.0.9`) なので、
/// 書き戻しでは元の表記から接頭辞 (`temurin-`) / セレクタ (`prefix:`) /
/// セグメント数 (`26` は 1 セグメントのまま) を復元する必要がある。
///
/// 元の表記を解釈できない場合は新バージョンをそのまま返す
/// (parse 側で弾かれる形式なので、通常この経路には来ない)。
pub(crate) fn format_mise_version(old_value: &str, new_version: &str) -> String {
    MiseVersionParser
        .parse(old_value)
        .and_then(|spec| spec.try_format_updated(new_version))
        .unwrap_or_else(|| new_version.to_string())
}

/// バージョン数値部の先頭にある数値セグメント数を数える。
///
/// `21.0.5` → 3、`26.7` → 2、`3.15.0rc1` → 3、`3.15-dev` → 2。
/// 数値セグメントが 1 つも無ければ `None`。
fn numeric_segment_count(version: &str) -> Option<usize> {
    let count = version
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .next()
        .unwrap_or_default()
        .split('.')
        .take_while(|segment| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()))
        .count();
    (count > 0).then_some(count)
}

/// 数値部が「完全なバージョン」か「前方一致セレクタ」かを判定する。
///
/// 数値セグメント以外 (プレリリース識別子やビルド識別子) を含む場合は、
/// 部分指定として扱うと元の識別子を落としてしまうため完全一致とみなす。
fn is_exact_version(version: &str, numeric_segments: usize) -> bool {
    let numeric_head_len = version
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .next()
        .unwrap_or_default()
        .len();
    let has_extra = numeric_head_len < version.len();
    has_extra || numeric_segments >= EXACT_SEGMENT_COUNT
}

impl VersionParser for MiseVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let raw = version_str.trim();
        if raw.is_empty() {
            return None;
        }

        // `latest` / `lts` / `system` は常に浮動するため更新しても意味が変わらない
        if FLOATING_ALIASES
            .iter()
            .any(|alias| raw.eq_ignore_ascii_case(alias))
        {
            return None;
        }

        // `ref:` / `path:` / `sub-N:` はバージョンではなく解決方法の指定
        if NON_VERSION_SELECTORS
            .iter()
            .any(|selector| raw.starts_with(selector))
        {
            return None;
        }

        // `prefix:` セレクタを剥がす (剥がした後も更新時に復元する)
        let (selector, body) = match raw.strip_prefix(PREFIX_SELECTOR) {
            Some(rest) => (PREFIX_SELECTOR, rest),
            None => ("", raw),
        };
        let body = body.trim();
        if body.is_empty() {
            return None;
        }

        // `prefix:latest` のようにセレクタの後ろが浮動別名でも更新対象外
        if FLOATING_ALIASES
            .iter()
            .any(|alias| body.eq_ignore_ascii_case(alias))
        {
            return None;
        }

        let (flavor, version) = split_mise_flavor(body)?;
        let numeric_segments = numeric_segment_count(version)?;

        let kind = if !selector.is_empty() {
            // 明示的な `prefix:` は常に前方一致
            VersionSpecKind::Prefix
        } else if is_exact_version(version, numeric_segments) {
            VersionSpecKind::Exact
        } else {
            VersionSpecKind::Prefix
        };

        let prefix = format!("{selector}{flavor}");
        let mut spec = VersionSpec::new(kind, raw, version);
        if !prefix.is_empty() {
            spec = spec.with_prefix(prefix);
        }
        Some(spec)
    }

    fn language(&self) -> Language {
        Language::Mise
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Option<VersionSpec> {
        MiseVersionParser.parse(input)
    }

    #[test]
    fn test_exact_version() {
        let spec = parse("26.7.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "26.7.0");
        assert_eq!(spec.prefix, None);
        assert_eq!(spec.format_updated("26.8.1"), "26.8.1");
    }

    #[test]
    fn test_partial_version_is_prefix_and_keeps_segments() {
        let one = parse("26").unwrap();
        assert_eq!(one.kind, VersionSpecKind::Prefix);
        assert_eq!(one.format_updated("27.1.0"), "27");

        let two = parse("26.7").unwrap();
        assert_eq!(two.kind, VersionSpecKind::Prefix);
        assert_eq!(two.format_updated("26.8.1"), "26.8");
    }

    #[test]
    fn test_explicit_prefix_selector_is_preserved() {
        let spec = parse("prefix:26").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Prefix);
        assert_eq!(spec.version, "26");
        assert_eq!(spec.prefix.as_deref(), Some("prefix:"));
        assert_eq!(spec.format_updated("27.1.0"), "prefix:27");
    }

    /// 3 セグメントでも `prefix:` が明示されていれば前方一致のまま保つ
    #[test]
    fn test_explicit_prefix_with_full_version() {
        let spec = parse("prefix:1.19.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Prefix);
        assert_eq!(spec.format_updated("1.24.3"), "prefix:1.24.3");
    }

    #[test]
    fn test_vendor_prefix_is_preserved() {
        let spec = parse("temurin-21.0.5").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "21.0.5");
        assert_eq!(spec.prefix.as_deref(), Some("temurin-"));
        assert_eq!(spec.format_updated("21.0.9"), "temurin-21.0.9");
    }

    #[test]
    fn test_multi_segment_vendor_prefix() {
        let spec = parse("graalvm-community-17.0.7").unwrap();
        assert_eq!(spec.version, "17.0.7");
        assert_eq!(spec.prefix.as_deref(), Some("graalvm-community-"));
        assert_eq!(spec.format_updated("17.0.9"), "graalvm-community-17.0.9");

        let plus = parse("truffleruby+graalvm-24.1.1").unwrap();
        assert_eq!(plus.version, "24.1.1");
        assert_eq!(plus.prefix.as_deref(), Some("truffleruby+graalvm-"));
    }

    #[test]
    fn test_vendor_prefix_with_partial_version() {
        let spec = parse("temurin-21").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Prefix);
        assert_eq!(spec.version, "21");
        assert_eq!(spec.prefix.as_deref(), Some("temurin-"));
        assert_eq!(spec.format_updated("22.0.3"), "temurin-22");
    }

    #[test]
    fn test_prefix_selector_with_vendor() {
        let spec = parse("prefix:temurin-21").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Prefix);
        assert_eq!(spec.version, "21");
        assert_eq!(spec.prefix.as_deref(), Some("prefix:temurin-"));
        assert_eq!(spec.format_updated("22.0.3"), "prefix:temurin-22");
    }

    /// 数値の後ろに識別子が続く形 (`3.15.0rc1`) は完全一致として扱い、
    /// 切り詰めで識別子を落とさない
    #[test]
    fn test_version_with_trailing_identifier_is_exact() {
        let spec = parse("3.15.0rc1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "3.15.0rc1");

        let dev = parse("3.15-dev").unwrap();
        assert_eq!(dev.kind, VersionSpecKind::Exact);
        assert_eq!(dev.version, "3.15-dev");
    }

    #[test]
    fn test_floating_aliases_are_skipped() {
        assert!(parse("latest").is_none());
        assert!(parse("lts").is_none());
        assert!(parse("system").is_none());
        assert!(parse("LATEST").is_none());
        assert!(parse("prefix:latest").is_none());
    }

    #[test]
    fn test_non_version_selectors_are_skipped() {
        assert!(parse("ref:master").is_none());
        assert!(parse("path:/opt/homebrew/opt/node@20").is_none());
        assert!(parse("sub-2:lts").is_none());
        assert!(parse("sub-0.1:latest").is_none());
    }

    #[test]
    fn test_versionless_vendor_is_skipped() {
        // 数値部が無いので更新先を決められない
        assert!(parse("jruby-dev").is_none());
        assert!(parse("truffleruby-dev").is_none());
        assert!(parse("").is_none());
        assert!(parse("   ").is_none());
    }

    #[test]
    fn test_split_mise_flavor() {
        assert_eq!(split_mise_flavor("26.7.0"), Some(("", "26.7.0")));
        assert_eq!(
            split_mise_flavor("temurin-21.0.5"),
            Some(("temurin-", "21.0.5"))
        );
        assert_eq!(
            split_mise_flavor("graalvm-community-17.0.7"),
            Some(("graalvm-community-", "17.0.7"))
        );
        // 数値部の後ろにハイフンが続いても最初の数値開始位置で切る
        assert_eq!(
            split_mise_flavor("temurin-21.0.5-b1"),
            Some(("temurin-", "21.0.5-b1"))
        );
        assert_eq!(split_mise_flavor("jruby-dev"), None);
        assert_eq!(split_mise_flavor(""), None);
    }

    #[test]
    fn test_language() {
        assert_eq!(MiseVersionParser.language(), Language::Mise);
    }
}
