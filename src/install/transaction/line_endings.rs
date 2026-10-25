//! Keep the pre-install newline convention when a manager rewrites a text file.
use super::*;

pub(super) fn preserve(record: &mut FileRecord) -> io::Result<()> {
    // Known binary manager files and arbitrary mise sidecar metadata are opaque.
    let path = &record.guards[0].alias;
    if path
        .extension()
        .is_some_and(|ext| ext == "jar" || ext == "lockb")
        || record.guards.iter().any(|guard| {
            guard.alias.ancestors().any(|path| {
                path.file_name().is_some_and(|name| name == "locks")
                    && path
                        .parent()
                        .and_then(Path::file_name)
                        .is_some_and(|name| name == "mise" || name == ".mise")
            })
        })
    {
        return Ok(());
    }
    let Some((before, after)) = record
        .before
        .bytes
        .as_deref()
        .zip(record.expected.bytes.as_deref())
    else {
        return Ok(());
    };
    let Some(bytes) = convert(before, after) else {
        return Ok(());
    };
    // Only a state already adopted from a manager command or an audit receipt
    // may be normalized. Never use normalized text for ownership comparisons.
    for guard in &record.guards {
        guard.check()?;
    }
    write_checked(
        &record.guards[0],
        &record.expected,
        &bytes,
        record.expected.permissions.as_ref(),
    )?;
    record.expected.bytes = Some(bytes);
    if !record.expected.matches(&read_record(record)?) {
        return Err(io::Error::other(
            "File changed after preserving line endings",
        ));
    }
    Ok(())
}

fn text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
        && !bytes.iter().enumerate().any(|(index, &byte)| {
            (byte.is_ascii_control() && !matches!(byte, b'\n' | b'\r' | b'\t'))
                || (byte == b'\r' && bytes.get(index + 1) != Some(&b'\n'))
        })
}

fn convert(before: &[u8], after: &[u8]) -> Option<Vec<u8>> {
    if before == after || !text(before) || !text(after) {
        return None;
    }
    let mut style = None;
    for (index, &byte) in before.iter().enumerate() {
        if byte == b'\n' {
            let crlf = index > 0 && before[index - 1] == b'\r';
            if style.is_some_and(|known| known != crlf) {
                return None;
            }
            style = Some(crlf);
        }
    }
    let crlf = style?;
    let mut converted = Vec::with_capacity(after.len());
    for &byte in after {
        if byte == b'\r' {
            continue;
        }
        if byte == b'\n' && crlf {
            converted.push(b'\r');
        }
        // A trailing newline is neither added nor removed.
        converted.push(byte);
    }
    (converted != after).then_some(converted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(path: &Path, before: Option<&[u8]>, after: &[u8]) -> FileRecord {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        if let Some(before) = before {
            fs::write(path, before).unwrap();
        }
        let guard = FileGuard::capture(path).unwrap();
        let before = guard.read().unwrap();
        fs::write(path, after).unwrap();
        let expected = guard.read().unwrap();
        FileRecord {
            guards: vec![guard],
            before,
            expected,
            conflict: None,
            parent_permissions: vec![],
        }
    }

    #[test]
    fn skips_new_files_binary_paths_and_mise_sidecars() {
        let temp = tempfile::tempdir().unwrap();
        let after = b"new\r\n";
        for path in [
            "gradle-wrapper.jar",
            "bun.lockb",
            ".mise/locks/mise/node",
            "mise/locks/config/node",
        ] {
            let path = temp.path().join(path);
            let mut record = record(&path, Some(b"old\n"), after);
            preserve(&mut record).unwrap();
            assert_eq!(fs::read(path).unwrap(), after);
        }
        let path = temp.path().join("new.lock");
        let mut record = record(&path, None, after);
        preserve(&mut record).unwrap();
        assert_eq!(fs::read(path).unwrap(), after);
    }

    #[test]
    fn refuses_even_a_newline_only_external_edit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Cargo.lock");
        let mut record = record(&path, Some(b"old\n"), b"new\r\n");
        // Normalizing first would mask this unowned change.
        fs::write(&path, b"new\n").unwrap();
        assert!(preserve(&mut record).is_err());
        assert_eq!(fs::read(path).unwrap(), b"new\n");
        assert_eq!(
            record.expected.bytes.as_deref(),
            Some(b"new\r\n".as_slice())
        );
    }

    #[test]
    fn converts_newlines_without_changing_other_bytes_or_the_final_newline() {
        for tail in ["", "\n"] {
            let after = format!("# 日本語\r\nversion = 4{tail}");
            assert_eq!(
                convert(b"old\n", after.as_bytes()).unwrap(),
                format!("# 日本語\nversion = 4{tail}").as_bytes()
            );
            let after = format!("# 日本語\nversion = 4{tail}");
            assert_eq!(
                convert(b"old\r\n", after.as_bytes()).unwrap(),
                format!("# 日本語\r\nversion = 4{}", tail.replace('\n', "\r\n")).as_bytes()
            );
        }
    }

    #[test]
    fn ambiguous_or_binary_input_is_left_untouched() {
        for before in [
            b"".as_slice(),
            b"no newline",
            b"mixed\r\nlines\n",
            b"old\rline",
            b"\xff\n",
            b"\0\n",
        ] {
            assert!(convert(before, b"new\r\n").is_none());
        }
        for after in [b"\xff\r\n".as_slice(), b"\0\r\n", b"new\rold\r\n", b"new\n"] {
            assert!(convert(b"old\n", after).is_none());
        }
    }
}
