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

pub(crate) fn replace_string_property_in_ranges(
    content: &str,
    mut ranges: Vec<(usize, usize)>,
    property_name: &str,
    transform: &mut impl FnMut(&str) -> Option<String>,
) -> Result<(String, bool), regex::Error> {
    let escaped_property = regex::escape(property_name);
    let pattern = format!(r#"("{}"\s*:\s*)"([^"]+)""#, escaped_property);
    let re = Regex::new(&pattern)?;

    let mut result = content.to_string();
    let mut updated = false;

    ranges.sort_by_key(|(start, _)| *start);
    for (start, end) in ranges.into_iter().rev() {
        let replaced = {
            let section = &result[start..end];
            re.replace_all(section, |caps: &regex::Captures| {
                let prefix = &caps[1];
                let old_value = &caps[2];
                if let Some(new_value) = transform(old_value) {
                    updated = true;
                    format!(r#"{}"{}""#, prefix, new_value)
                } else {
                    caps[0].to_string()
                }
            })
            .into_owned()
        };
        result.replace_range(start..end, &replaced);
    }

    Ok((result, updated))
}
