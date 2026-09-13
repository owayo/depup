use regex::Regex;

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

pub(crate) fn direct_child_object_section_ranges(
    content: &str,
    parent_ranges: &[(usize, usize)],
    section_names: Option<&[&str]>,
) -> Vec<(usize, usize)> {
    let bytes = content.as_bytes();
    let mut ranges = Vec::new();

    for &(start, end) in parent_ranges {
        let mut depth = 0usize;
        let mut i = start;

        while i < end && i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    let Some(string_end) = find_json_string_end(bytes, i) else {
                        break;
                    };

                    if depth == 0 {
                        let key = &content[i + 1..string_end];
                        let mut j = skip_json_ws(bytes, string_end + 1);
                        if j < end && bytes[j] == b':' {
                            j = skip_json_ws(bytes, j + 1);
                            if j < end
                                && bytes[j] == b'{'
                                && section_names.is_none_or(|names| names.contains(&key))
                                && let Some(object_end) = find_matching_json_object_end(bytes, j)
                                && object_end <= end
                            {
                                ranges.push((j + 1, object_end));
                                i = object_end + 1;
                                continue;
                            }
                        }
                    }

                    i = string_end + 1;
                }
                b'{' => {
                    depth += 1;
                    i += 1;
                }
                b'}' => {
                    depth = depth.saturating_sub(1);
                    i += 1;
                }
                _ => i += 1,
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
    let mut ranges = Vec::new();
    let mut depth = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let Some(end) = find_json_string_end(bytes, i) else {
                    break;
                };

                if depth == 1 {
                    let key = &content[i + 1..end];
                    let mut j = skip_json_ws(bytes, end + 1);
                    if j < bytes.len() && bytes[j] == b':' {
                        j = skip_json_ws(bytes, j + 1);
                        if j < bytes.len()
                            && bytes[j] == b'{'
                            && section_names.contains(&key)
                            && let Some(object_end) = find_matching_json_object_end(bytes, j)
                        {
                            ranges.push((j + 1, object_end));
                            i = object_end + 1;
                            continue;
                        }
                    }
                }

                i = end + 1;
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            _ => i += 1,
        }
    }

    ranges
}

pub(crate) fn replace_string_property_in_top_level_sections(
    content: &str,
    section_names: &[&str],
    property_name: &str,
    mut transform: impl FnMut(&str) -> Option<String>,
) -> Result<(String, bool), regex::Error> {
    let ranges = top_level_object_section_ranges(content, section_names);
    replace_string_property_in_ranges(content, ranges, property_name, &mut transform)
}

/// JSON のキー・値ペア (どちらも文字列リテラル) を、エスケープを含んだまま捉える。
/// 一致したキー・値は生テキストのままなので、比較・書き戻しの前後で必ずデコード /
/// エンコードを通す。
const JSON_STRING_PROPERTY_PATTERN: &str = r#"("(?:[^"\\]|\\.)*")(\s*:\s*)("(?:[^"\\]|\\.)*")"#;

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
    mut ranges: Vec<(usize, usize)>,
    property_name: &str,
    transform: &mut impl FnMut(&str) -> Option<String>,
) -> Result<(String, bool), regex::Error> {
    // キーと値は生テキストのリテラルではなく、JSON 文字列としてデコードしてから
    // 比較・変換する。PHP の `json_encode` は既定で `/` を `\/` へエスケープするため
    // (`"monolog\/monolog"`)、リテラル一致では composer.json の依存キーに 1 つも
    // 当たらず、judge が「更新あり」と報告した後に書き込みが必ず失敗していた
    // (composer のパッケージ名は必ず `/` を含むので全依存が失敗する)。
    // package.json の scoped 名 (`"@types\/node"`) や、Go の `encoding/json` が付ける
    // `<` のような別表記も同じ経路で救われる。
    let re = Regex::new(JSON_STRING_PROPERTY_PATTERN)?;

    let mut result = content.to_string();
    let mut updated = false;

    ranges.sort_by_key(|(start, _)| *start);
    for (start, end) in ranges.into_iter().rev() {
        let replaced = {
            let section = &result[start..end];
            re.replace_all(section, |caps: &regex::Captures| {
                let raw_key = &caps[1];
                let separator = &caps[2];
                let raw_value = &caps[3];
                // キーがデコードできない / 対象パッケージでない場合は素通しする
                if decode_json_string(raw_key).as_deref() != Some(property_name) {
                    return caps[0].to_string();
                }
                let Some(old_value) = decode_json_string(raw_value) else {
                    return caps[0].to_string();
                };
                let Some(new_value) = transform(&old_value) else {
                    return caps[0].to_string();
                };
                let Some(encoded) = encode_json_string(&new_value) else {
                    return caps[0].to_string();
                };
                updated = true;
                format!("{}{}{}", raw_key, separator, encoded)
            })
            .into_owned()
        };
        result.replace_range(start..end, &replaced);
    }

    Ok((result, updated))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        )
        .unwrap();

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
        )
        .unwrap();
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
