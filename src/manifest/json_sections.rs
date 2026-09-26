fn find_json_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut escaped = false;
    let mut i = start + 1;
    while i < bytes.len() {
        let byte = bytes[i];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn skip_json_ws(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn find_matching_json_object_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = find_json_string_end(bytes, i)? + 1;
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// オブジェクト本文の直下にあるキー（引用符込み）と値の開始位置を返す。
/// オブジェクトと配列の両方を追跡し、文字列内の括弧は構造として扱わない。
fn direct_child_properties(content: &str, start: usize, end: usize) -> Vec<(&str, usize)> {
    let bytes = content.as_bytes();
    let end = end.min(bytes.len());
    let mut properties = Vec::new();
    let mut depth = 0usize;
    let mut i = start;

    while i < end {
        match bytes[i] {
            b'"' => {
                let Some(string_end) = find_json_string_end(bytes, i).filter(|&pos| pos < end)
                else {
                    break;
                };
                if depth == 0 {
                    let mut j = skip_json_ws(bytes, string_end + 1);
                    if j < end && bytes[j] == b':' {
                        j = skip_json_ws(bytes, j + 1);
                        if j < end {
                            properties.push((&content[i..=string_end], j));
                        }
                    }
                }
                i = string_end + 1;
            }
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            _ => i += 1,
        }
    }

    properties
}

pub(crate) fn direct_child_object_section_ranges(
    content: &str,
    parent_ranges: &[(usize, usize)],
    section_names: Option<&[&str]>,
) -> Vec<(usize, usize)> {
    let bytes = content.as_bytes();
    let mut ranges = Vec::new();
    for &(start, end) in parent_ranges {
        for (raw_key, value_start) in direct_child_properties(content, start, end) {
            if bytes[value_start] == b'{'
                && let Some(key) = decode_json_string(raw_key)
                && section_names.is_none_or(|names| names.contains(&key.as_str()))
                && let Some(object_end) = find_matching_json_object_end(bytes, value_start)
                && object_end <= end
            {
                ranges.push((value_start + 1, object_end));
            }
        }
    }
    ranges
}

pub(crate) fn top_level_object_section_ranges(
    content: &str,
    section_names: &[&str],
) -> Vec<(usize, usize)> {
    let bytes = content.as_bytes();
    let start = skip_json_ws(bytes, 0);
    if bytes.get(start) != Some(&b'{') {
        return Vec::new();
    }
    let Some(end) = find_matching_json_object_end(bytes, start) else {
        return Vec::new();
    };
    direct_child_object_section_ranges(content, &[(start + 1, end)], Some(section_names))
}

pub(crate) fn replace_string_property_in_top_level_sections(
    content: &str,
    section_names: &[&str],
    property_name: &str,
    mut transform: impl FnMut(&str) -> Option<String>,
) -> (String, bool) {
    let ranges = top_level_object_section_ranges(content, section_names);
    replace_string_property_in_ranges(content, ranges, property_name, &mut transform)
}

/// JSON 文字列リテラル (引用符込み) をデコードする
fn decode_json_string(raw: &str) -> Option<String> {
    serde_json::from_str::<String>(raw).ok()
}

/// 文字列を JSON 文字列リテラル (引用符込み) へエンコードする
fn encode_json_string(value: &str) -> Option<String> {
    serde_json::to_string(value).ok()
}

pub(crate) fn replace_string_property_in_ranges(
    content: &str,
    ranges: Vec<(usize, usize)>,
    property_name: &str,
    transform: &mut impl FnMut(&str) -> Option<String>,
) -> (String, bool) {
    let bytes = content.as_bytes();
    let mut replacements = Vec::new();
    for (start, end) in ranges {
        for (raw_key, value_start) in direct_child_properties(content, start, end) {
            // parse と同じく直下の文字列値だけを対象にし、キーもデコードして照合する。
            if decode_json_string(raw_key).as_deref() == Some(property_name)
                && bytes[value_start] == b'"'
                && let Some(value_end) = find_json_string_end(bytes, value_start)
                && value_end < end
                && let Some(old_value) = decode_json_string(&content[value_start..=value_end])
                && let Some(new_value) = transform(&old_value)
                && let Some(encoded) = encode_json_string(&new_value)
            {
                replacements.push((value_start, value_end + 1, encoded));
            }
        }
    }

    // 元の byte offset がずれないよう、ファイルの後方から値だけを差し替える。
    replacements.sort_by_key(|(start, _, _)| *start);
    let updated = !replacements.is_empty();
    let mut result = content.to_string();
    for (start, end, encoded) in replacements.into_iter().rev() {
        result.replace_range(start..end, &encoded);
    }
    (result, updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        ManifestParser, composer_json::ComposerJsonParser, package_json::PackageJsonParser,
    };

    #[test]
    fn test_escaped_section_names_are_updatable() {
        let cases: &[(&dyn ManifestParser, &str, &str)] = &[
            (
                &PackageJsonParser,
                r#"{"depend\u0065ncies":{"foo":"^1.0.0"}}"#,
                "foo",
            ),
            (
                &PackageJsonParser,
                r#"{"devDepend\u0065ncies":{"foo":"^1.0.0"}}"#,
                "foo",
            ),
            (
                &PackageJsonParser,
                r#"{"catal\u006fg":{"foo":"^1.0.0"}}"#,
                "foo",
            ),
            (
                &PackageJsonParser,
                r#"{"catal\u006fgs":{"default":{"foo":"^1.0.0"}}}"#,
                "foo",
            ),
            (
                &PackageJsonParser,
                r#"{"worksp\u0061ces":{"catal\u006fg":{"foo":"^1.0.0"}}}"#,
                "foo",
            ),
            (
                &PackageJsonParser,
                r#"{"worksp\u0061ces":{"catal\u006fgs":{"default":{"foo":"^1.0.0"}}}}"#,
                "foo",
            ),
            (
                &ComposerJsonParser,
                r#"{"requ\u0069re":{"vendor/foo":"^1.0.0"}}"#,
                "vendor/foo",
            ),
            (
                &ComposerJsonParser,
                r#"{"requ\u0069re-dev":{"vendor/foo":"^1.0.0"}}"#,
                "vendor/foo",
            ),
        ];
        for &(parser, content, package) in cases {
            let deps = parser.parse(content).unwrap();
            assert_eq!(deps.len(), 1, "{content}");
            let updated = parser.update_version(content, package, "2.0.0").unwrap();
            assert_eq!(updated, content.replace("^1.0.0", "^2.0.0"));
            assert_eq!(parser.parse(&updated).unwrap()[0].version(), "2.0.0");
        }
    }

    #[test]
    fn test_replacement_leaves_nested_non_dependency_values_untouched() {
        let cases: &[(&dyn ManifestParser, &str, &str)] = &[
            (
                &PackageJsonParser,
                r#"{"dependencies":{"foo":"^1.0.0","object":{"foo":"^0.1.0"},"array":[{"foo":"^0.2.0"}]}}"#,
                "foo",
            ),
            (
                &PackageJsonParser,
                r#"{"workspaces":{"catalogs":{"default":{"foo":"^1.0.0","object":{"foo":"^0.1.0"},"array":[{"foo":"^0.2.0"}]}}}}"#,
                "foo",
            ),
            (
                &ComposerJsonParser,
                r#"{"require":{"vendor/foo":"^1.0.0","object":{"vendor/foo":"^0.1.0"},"array":[{"vendor/foo":"^0.2.0"}]}}"#,
                "vendor/foo",
            ),
        ];
        for &(parser, content, package) in cases {
            assert_eq!(parser.parse(content).unwrap().len(), 1);
            let updated = parser.update_version(content, package, "2.0.0").unwrap();
            assert_eq!(updated, content.replace("^1.0.0", "^2.0.0"));

            // 直下に宣言がなければ、ネストした同名キーを更新成功と報告しない。
            let nested_only = content.replace(&format!(r#""{package}":"^1.0.0","#), "");
            assert!(parser.parse(&nested_only).unwrap().is_empty());
            assert!(
                parser
                    .update_version(&nested_only, package, "2.0.0")
                    .is_err()
            );
        }
    }

    #[test]
    fn test_escaped_json_update_preserves_crlf_and_key_spelling() {
        let content = "{\r\n\t\"requ\\u0069re\" : { \"vendor\\/foo\" : \"\\u005e1.0.0\" }\r\n}";
        let updated = ComposerJsonParser
            .update_version(content, "vendor/foo", "2.0.0")
            .unwrap();
        assert_eq!(updated, content.replace(r#""\u005e1.0.0""#, r#""^2.0.0""#));
        assert_eq!(
            ComposerJsonParser.parse(&updated).unwrap()[0].version(),
            "2.0.0"
        );
    }

    #[test]
    fn test_top_level_ranges_ignore_braces_and_escaped_quotes_in_strings() {
        let content = r#"{
  "description": "文字列内の { dependencies } と \"引用符\" は構造ではない",
  "dependencies": { "serde": "1.0" },
  "nested": { "dependencies": { "serde": "0.9" } }
}"#;

        let ranges = top_level_object_section_ranges(content, &["dependencies"]);

        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].0..ranges[0].1], r#" "serde": "1.0" "#);
    }

    #[test]
    fn test_direct_child_ranges_only_return_selected_sections() {
        let content = r#"{
  "workspaces": {
    "catalog": { "react": "^19.0.0" },
    "ignored": { "react": "^18.0.0" }
  }
}"#;
        let parents = top_level_object_section_ranges(content, &["workspaces"]);

        let ranges = direct_child_object_section_ranges(content, &parents, Some(&["catalog"]));

        assert_eq!(ranges.len(), 1);
        assert_eq!(
            &content[ranges[0].0..ranges[0].1],
            r#" "react": "^19.0.0" "#
        );
    }

    #[test]
    fn test_replace_property_updates_multiple_ranges_from_the_end() {
        let content = r#"{
  "dependencies": { "@scope/pkg": "^1.0.0" },
  "devDependencies": { "@scope/pkg": "~1.0.0" },
  "overrides": { "@scope/pkg": "1.0.0" }
}"#;

        let (updated, changed) = replace_string_property_in_top_level_sections(
            content,
            &["dependencies", "devDependencies"],
            "@scope/pkg",
            |old| Some(old.replace("1.0.0", "2.0.0")),
        );

        assert!(changed);
        assert!(updated.contains(r#""@scope/pkg": "^2.0.0""#));
        assert!(updated.contains(r#""@scope/pkg": "~2.0.0""#));
        assert!(updated.contains(r#""overrides": { "@scope/pkg": "1.0.0" }"#));
    }

    #[test]
    fn test_malformed_object_returns_no_range_without_panicking() {
        let content = r#"{ "dependencies": { "serde": "1.0" "#;

        let ranges = top_level_object_section_ranges(content, &["dependencies"]);

        assert!(ranges.is_empty());
    }

    #[test]
    fn test_multibyte_prefix_keeps_byte_offsets_aligned() {
        // 対象セクションより手前に多バイト文字があると、byte offset と char index を
        // 取り違えた実装では範囲が数バイトずれて文字境界違反で panic するか、
        // 別のキーを書き換えてしまう
        let content = r#"{
  "description": "日本語の説明テキスト — em dash と絵文字 🎉 を含む",
  "dependencies": { "serde": "1.0" }
}"#;

        let ranges = top_level_object_section_ranges(content, &["dependencies"]);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].0..ranges[0].1], r#" "serde": "1.0" "#);

        let (updated, changed) = replace_string_property_in_top_level_sections(
            content,
            &["dependencies"],
            "serde",
            |_| Some("2.0".to_string()),
        );
        assert!(changed);
        assert!(updated.contains(r#""serde": "2.0""#));
        // 手前の多バイト文字列は無傷であること
        assert!(updated.contains("日本語の説明テキスト — em dash と絵文字 🎉 を含む"));
    }

    #[test]
    fn test_trailing_escaped_backslash_does_not_swallow_closing_quote() {
        // `"...\\"` は「エスケープされたバックスラッシュ + 閉じ引用符」であり、
        // エスケープ状態を持ち越すと閉じ引用符を食って以降の構造解析が崩れる
        let content = r#"{
  "description": "windows path C:\\",
  "dependencies": { "serde": "1.0" }
}"#;

        let ranges = top_level_object_section_ranges(content, &["dependencies"]);

        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].0..ranges[0].1], r#" "serde": "1.0" "#);
    }

    #[test]
    fn test_empty_and_unclosed_input_are_handled() {
        assert!(top_level_object_section_ranges("", &["dependencies"]).is_empty());
        // 閉じない文字列リテラルで break しても panic しない
        assert!(top_level_object_section_ranges(r#"{ "depend"#, &["dependencies"]).is_empty());
        assert!(
            direct_child_object_section_ranges("", &[(0, 0)], None).is_empty(),
            "空入力の親範囲でも panic しない"
        );
    }
}
