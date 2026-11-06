//! 比較子の組の中の `^` / `~` / `~>` / x-range / 裸の版が持つ暗黙の上限 (#99)。
//!
//! `>=1.2.5 ^1.2.0` の `^1.2.0` は `<2.0.0`、`>=1.2.5, ~1.2, <2.0` の `~1.2` は `<1.3.0` を
//! 意味する。明示的な `<` / `<=` だけを上限として読むと、下限だけを進めて充足できない範囲
//! (`>=2.0.0 ^1.2.0`) を書き込む。対象は比較子を空白かカンマで並べる npm・Composer・Cargo・
//! RubyGems で、PEP 440 の `~=` / `==1.2.*` は `extract_upper_bound` が別に読む。

use super::{increment_release_prefix, normalize_bound_version};
use crate::domain::{Language, range_lower_bound_span};

/// 比較子の組の暗黙の上限 (版, 包含か) を列挙する。
///
/// 書き換え (`format_range_like`) が置き換える比較子 (先頭の包含下限) は除く。置き換えた後は
/// 新しい版の比較子になるので (`^1.2.5 <3.0.0` は `^2.0.0 <3.0.0` になる)、その上限で候補を
/// 絞ると、単独の `^1.2.5` なら選べる版を選べなくなる。OR (`||` / `|`) とハイフンの範囲
/// (`1.2.3 - 2.3.4`) は比較子の組ではないので扱わない。
pub(super) fn implicit_upper_bounds(raw: &str, language: Language) -> Vec<(String, bool)> {
    if !matches!(
        language,
        Language::Node | Language::Php | Language::Rust | Language::Ruby
    ) || raw.contains('|')
        || has_hyphen_range(raw)
    {
        return Vec::new();
    }
    let replaced = range_lower_bound_span(raw);
    comparators(raw)
        .into_iter()
        .filter(|comparator| {
            replaced.is_none_or(|(start, _)| {
                !(comparator.version_start..comparator.version_end).contains(&start)
            })
        })
        .filter_map(|comparator| {
            implicit_upper_bound(
                comparator.operator,
                &raw[comparator.version_start..comparator.version_end],
                language,
            )
        })
        .collect()
}

/// 比較子 1 つ。`operator` は `^` / `~` / `~>` / `=` / `>=` などで、裸の版なら空
struct Comparator<'a> {
    operator: &'a str,
    version_start: usize,
    version_end: usize,
}

/// 比較子の組を比較子に分ける。区切りは空白とカンマで、演算子と版の間の空白
/// (`>= 1.2.3` / `~> 2.2`) は比較子の内側として読む。
fn comparators(raw: &str) -> Vec<Comparator<'_>> {
    let bytes = raw.as_bytes();
    let mut comparators = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() || bytes[index] == b',' {
            index += 1;
            continue;
        }
        let operator_start = index;
        while index < bytes.len() && matches!(bytes[index], b'<' | b'>' | b'=' | b'^' | b'~' | b'!')
        {
            index += 1;
        }
        let operator = &raw[operator_start..index];
        if !operator.is_empty() {
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
        }
        let version_start = index;
        while index < bytes.len() && is_version_byte(bytes[index]) {
            index += 1;
        }
        if index == version_start {
            // 演算子でも版でもない文字 (`^1.0@dev` の `@` など) は読み飛ばす
            if index == operator_start {
                index += raw[index..].chars().next().map_or(1, char::len_utf8);
            }
            continue;
        }
        comparators.push(Comparator {
            operator,
            version_start,
            version_end: index,
        });
    }
    comparators
}

fn is_version_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'*' | b'+' | b'-' | b'_')
}

/// ハイフンの範囲 (`1.2.3 - 2.3.4`) を含むか。npm と Composer はハイフンの前後に空白が要る。
fn has_hyphen_range(raw: &str) -> bool {
    raw.as_bytes().windows(3).any(|window| {
        window[0].is_ascii_whitespace() && window[1] == b'-' && window[2].is_ascii_whitespace()
    })
}

/// 版の数値の要素
struct Release<'a> {
    /// 先頭から続く数値の要素。ワイルドカードの要素とその後は含まない
    numbers: Vec<&'a str>,
    /// ワイルドカード (`x` / `X` / `*`) の要素があるか
    wildcard: bool,
}

fn release(version: &str, language: Language) -> Option<Release<'_>> {
    let version = version.strip_prefix(['v', 'V']).unwrap_or(version);
    // SemVer 系はプレリリースとビルドメタデータの前まで (`1.2.3-rc.1+b5` は `1.2.3`)。
    // RubyGems は数値でない要素 (`2.2.0.rc1` の `rc1`) からがプレリリース
    let core = match language {
        Language::Ruby => version,
        _ => version.split(['-', '+']).next().unwrap_or(version),
    };
    let mut numbers = Vec::new();
    let mut wildcard = false;
    for segment in core.split('.') {
        if !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()) {
            numbers.push(segment);
        } else if language != Language::Ruby && matches!(segment, "x" | "X" | "*") {
            wildcard = true;
            break;
        } else if language == Language::Ruby && !numbers.is_empty() {
            break;
        } else {
            return None;
        }
    }
    Some(Release { numbers, wildcard })
}

/// 比較子 1 つの暗黙の上限。上限を持たない比較子 (`>=` など。明示的な `<` / `<=` は
/// `extract_upper_bound` が読む) と、読めない版は `None`。
fn implicit_upper_bound(
    operator: &str,
    version: &str,
    language: Language,
) -> Option<(String, bool)> {
    let release = release(version, language)?;
    let exclusive = |upper: Option<String>| upper.map(|upper| (upper, false));
    let exact = || Some((normalize_bound_version(version), true));
    match (language, operator) {
        (Language::Node | Language::Php | Language::Rust, "^") => {
            exclusive(caret_upper(&release.numbers))
        }
        // node-semver は `~>` を `~` と同じに読む
        (Language::Node | Language::Rust, "~") | (Language::Node, "~>") => {
            exclusive(tilde_upper(&release.numbers))
        }
        (Language::Php, "~") | (Language::Ruby, "~>") => {
            exclusive(pessimistic_upper(&release.numbers))
        }
        // Cargo の裸の版は `^` と同じ
        (Language::Rust, "") if !release.wildcard => exclusive(caret_upper(&release.numbers)),
        // npm と Cargo の部分版・ワイルドカードは x-range (`1.2` / `1.2.x` は `<1.3.0`)、
        // 完全な版は完全一致
        (Language::Node | Language::Rust, "" | "=") => {
            if release.wildcard || release.numbers.len() < 3 {
                exclusive(prefix_upper(&release.numbers))
            } else {
                exact()
            }
        }
        // Composer と RubyGems の部分版は完全一致 (`1.2` は `1.2.0`)。ワイルドカードだけが範囲
        (Language::Php, "" | "=" | "==") | (Language::Ruby, "" | "=") => {
            if release.wildcard {
                exclusive(prefix_upper(&release.numbers))
            } else {
                exact()
            }
        }
        _ => None,
    }
}

/// 書いた要素の最後を 1 つ上げた排他的な上限 (`1.2` → `1.3`)。要素がなければ (`*`) 上限なし
fn prefix_upper(numbers: &[&str]) -> Option<String> {
    if numbers.is_empty() {
        return None;
    }
    increment_release_prefix(&numbers.join("."))
}

/// `^` の上限。左端の 0 でない要素を上げる。書いた要素がすべて 0 なら最後に書いた要素を
/// 上げる (`^1.2.3` → `<2`、`^0.2.3` → `<0.3`、`^0.0.3` → `<0.0.4`、`^0.0` → `<0.1`、`^0` → `<1`)
fn caret_upper(numbers: &[&str]) -> Option<String> {
    let last = numbers.len().checked_sub(1)?;
    let index = numbers
        .iter()
        .position(|number| number.bytes().any(|byte| byte != b'0'))
        .unwrap_or(last);
    prefix_upper(&numbers[..=index])
}

/// npm と Cargo の `~` の上限。マイナーまで書けばマイナーを、メジャーだけならメジャーを上げる
/// (`~1.2.3` / `~1.2` → `<1.3`、`~1` → `<2`)
fn tilde_upper(numbers: &[&str]) -> Option<String> {
    prefix_upper(&numbers[..numbers.len().min(2)])
}

/// Composer の `~` と RubyGems の `~>` の上限。最後に書いた要素を落とし、その前の要素を上げる
/// (`~1.2` → `<2`、`~1.2.3` → `<1.3`)。要素が 1 つなら、それを上げる (`~1` → `<2`)
fn pessimistic_upper(numbers: &[&str]) -> Option<String> {
    let kept = if numbers.len() > 1 {
        numbers.len() - 1
    } else {
        numbers.len()
    };
    prefix_upper(&numbers[..kept])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(raw: &str, language: Language) -> Vec<(String, bool)> {
        implicit_upper_bounds(raw, language)
    }

    fn exclusive(version: &str) -> Vec<(String, bool)> {
        vec![(version.to_string(), false)]
    }

    #[test]
    fn caret_tilde_and_pessimistic_bounds_follow_each_ecosystem() {
        for (raw, language, expected) in [
            (">=1.2.5 ^1.2.0", Language::Node, exclusive("2")),
            (">=0.2.5 ^0.2.0", Language::Node, exclusive("0.3")),
            (">=0.0.3 ^0.0.3", Language::Rust, exclusive("0.0.4")),
            (">=0.0.1, ^0.0, <1", Language::Rust, exclusive("0.1")),
            (">=0.1 ^0", Language::Php, exclusive("1")),
            (">=1.2.5 ^1.x", Language::Node, exclusive("2")),
            // npm と Cargo の `~` はマイナーまでを固定する
            (">=1.2.5 ~1.2", Language::Node, exclusive("1.3")),
            (">=1.2.5 ~>1.2.3", Language::Node, exclusive("1.3")),
            (">=1.2.5, ~1, <3", Language::Rust, exclusive("2")),
            // Composer の `~` と RubyGems の `~>` は最後に書いた要素だけが動く
            (">=1.2.5 ~1.2", Language::Php, exclusive("2")),
            (">=1.2.5 ~1.2.3", Language::Php, exclusive("1.3")),
            (">= 2.2.5, ~> 2.2", Language::Ruby, exclusive("3")),
            (">= 2.2.5, ~> 2.2.0.rc1", Language::Ruby, exclusive("2.3")),
            (">= 2.2.5, ~> 2", Language::Ruby, exclusive("3")),
            // プレリリースとビルドメタデータは上限の位置に関わらない
            (">=1.2.5 ^1.2.0-rc.1+build", Language::Node, exclusive("2")),
        ] {
            assert_eq!(bounds(raw, language), expected, "{raw} ({language:?})");
        }
    }

    #[test]
    fn bare_versions_and_wildcards_follow_each_ecosystem() {
        for (raw, language, expected) in [
            // npm の部分版とワイルドカードは x-range、完全な版は完全一致
            (">=1.2.5 1.x", Language::Node, exclusive("2")),
            (">=1.2.5 1.2", Language::Node, exclusive("1.3")),
            (
                ">=1.0.0 =1.2.3",
                Language::Node,
                vec![("1.2.3".to_string(), true)],
            ),
            // Cargo の裸の版は `^`、`=` の部分版は x-range
            (">=1.2.5, 1.3, <3", Language::Rust, exclusive("2")),
            (">=1.2.5, 1.2.*, <3", Language::Rust, exclusive("1.3")),
            (">=1.2.5, =1.2, <3", Language::Rust, exclusive("1.3")),
            // Composer の部分版は完全一致で、ワイルドカードだけが範囲
            (">=1.0 1.2", Language::Php, vec![("1.2".to_string(), true)]),
            (">=1.0 1.2.*", Language::Php, exclusive("1.3")),
            (
                ">= 1.0, = 1.2",
                Language::Ruby,
                vec![("1.2".to_string(), true)],
            ),
            // `*` は上限を持たない
            (">=1.0.0 *", Language::Node, vec![]),
        ] {
            assert_eq!(bounds(raw, language), expected, "{raw} ({language:?})");
        }
    }

    #[test]
    fn the_replaced_lower_bound_and_explicit_bounds_are_not_implicit_bounds() {
        for (raw, language) in [
            // 書き換えで置き換わる先頭の比較子 (`^` / 裸の版) は新しい版になる
            ("^1.2.5 <3.0.0", Language::Node),
            ("^1.2.5, <3", Language::Rust),
            ("1.2.3 <2.0.0", Language::Node),
            ("~1.2 >=1.2.5", Language::Node),
            // 明示的な上限と下限だけの組
            (">=1.0.0 <2.0.0", Language::Node),
            (">= 1.2, < 2.0", Language::Ruby),
            // 比較子の組でない形
            ("1.2.3 - 2.3.4", Language::Node),
            (">=1.0 <1.1 || ^2.0", Language::Php),
            ("^1.0 | ^2.0", Language::Php),
            // PEP 440 は extract_upper_bound が別に読む
            (">=1.0, ~=1.2", Language::Python),
        ] {
            assert_eq!(bounds(raw, language), vec![], "{raw} ({language:?})");
        }
        // 置き換わらない 2 つ目以降の `^` は上限になる
        assert_eq!(bounds("^1.2.5 ^1.3.0 <3", Language::Node), exclusive("2"));
    }
}
