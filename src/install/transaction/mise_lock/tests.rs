use super::*;

/// mise.lock の内容を読み、エントリを返す。
fn entries(content: &str) -> Vec<Entry> {
    parse(Some(content.as_bytes())).unwrap().1
}

/// mise.lock が無い (`None`) か空なら、保つ platform の無い lock として読む。
#[test]
fn parse_treats_a_missing_or_empty_lock_as_having_no_entries() {
    for bytes in [None, Some(&b""[..])] {
        let (lock, entries) = parse(bytes).unwrap();
        assert!(entries.is_empty());
        assert_eq!(
            lock,
            Lock {
                format: None,
                resolutions: vec![],
                metadata: vec![],
            }
        );
    }
}

/// platform の情報を保てない mise.lock は、理由ごとの文言のエラーにする。
#[test]
fn parse_rejects_locks_whose_platforms_cannot_be_preserved() {
    for (content, expected) in [
        (
            "[tools\n",
            "Cannot parse mise lock for platform preservation",
        ),
        ("tools = 1\n", "Invalid mise lock tools"),
        (
            "[[tools.\"-node\"]]\nversion = \"1.0.0\"\n",
            "Invalid tool name in mise lock",
        ),
        (
            "[[tools.\"\"]]\nversion = \"1.0.0\"\n",
            "Invalid tool name in mise lock",
        ),
        (
            "[[tools.\"no\\u0007de\"]]\nversion = \"1.0.0\"\n",
            "Invalid tool name in mise lock",
        ),
        (
            "[tools]\nnode = \"1.0.0\"\n",
            "Invalid mise lock tool versions",
        ),
        ("[tools]\nnode = [\"1.0.0\"]\n", "Invalid mise lock entry"),
        (
            "[[tools.node]]\nbackend = \"core:node\"\n",
            "Mise lock entry has no resolved version",
        ),
        (
            "[[tools.node]]\nversion = \"\"\n",
            "Mise lock entry has no resolved version",
        ),
        (
            "[[tools.node]]\nversion = 1\n",
            "Mise lock entry has no resolved version",
        ),
        (
            "[[tools.node]]\nversion = \"1.0.0\"\nplatforms = 1\n",
            "Invalid mise lock platforms",
        ),
        (
            "[[tools.node]]\nversion = \"1.0.0\"\n\"platforms.linux x64\" = { url = \"u\" }\n",
            "Invalid mise lock platform name",
        ),
        (
            "[[tools.node]]\nversion = \"1.0.0\"\nplatforms = { \"linux/x64\" = { url = \"u\" } }\n",
            "Invalid mise lock platform name",
        ),
        (
            "[[tools.node]]\nversion = \"1.0.0\"\n\"platforms.linux-x64\" = \"u\"\n",
            "Invalid mise lock platform metadata",
        ),
    ] {
        assert_eq!(
            parse(Some(content.as_bytes())).map(|_| ()),
            Err(expected.to_string()),
            "{content}"
        );
    }
    assert_eq!(
        parse(Some(b"\xff")).map(|_| ()),
        Err("Mise lock is not UTF-8".to_string())
    );
}

/// platform の情報は、引用符付きのキー (`"platforms.<名前>"`) と `platforms` の表のどちらからも読み、
/// 解決の結果 (resolution) からは外す。url と checksum は、空白だけの値を無いものとみなす。
#[test]
fn parse_reads_both_platform_forms_and_ignores_blank_metadata() {
    let (lock, entries) = parse(Some(
        concat!(
            "lockfile_version = 1\n",
            "[[tools.node]]\n",
            "version = \"24.0.0\"\n",
            "backend = \"core:node\"\n",
            "\"platforms.linux-x64\" = { url = \"https://example.com/node\", checksum = \" \" }\n",
            "[tools.node.platforms.macos_arm64]\n",
            "checksum = \"sha256:example\"\n",
        )
        .as_bytes(),
    ))
    .unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.tool, "node");
    assert_eq!(
        entry.platforms,
        BTreeMap::from([
            ("linux-x64".to_string(), (true, false)),
            ("macos_arm64".to_string(), (false, true)),
        ])
    );
    assert_eq!(
        entry.metadata.keys().collect::<Vec<_>>(),
        ["linux-x64", "macos_arm64"]
    );
    let resolution: toml::Value =
        toml::from_str("version = \"24.0.0\"\nbackend = \"core:node\"\n").unwrap();
    assert_eq!(entry.resolution, resolution);
    assert_eq!(lock.format, Some(toml::Value::Integer(1)));
    assert_eq!(lock.resolutions, vec![("node".to_string(), resolution)]);
}

/// install の後のエントリから、前のエントリに当たるものを、完全に同じもの → specifiers が同じもの →
/// 同じツールで識別の項目 (version と specifiers 以外。空の options は無いものとみなす) が同じ唯一のもの、
/// の順に選ぶ。決められなければエラーにする。
#[test]
fn matching_prefers_the_exact_entry_then_the_same_specifiers_then_the_only_candidate() {
    let old = entries(
        "[[tools.node]]\nversion = \"1.0.0\"\nbackend = \"core:node\"\nspecifiers = [\"^1\"]\n",
    )
    .remove(0);
    let after = entries(concat!(
        "[[tools.node]]\nversion = \"2.0.0\"\nbackend = \"core:node\"\nspecifiers = [\"^2\"]\n",
        "[[tools.node]]\nversion = \"1.0.0\"\nbackend = \"core:node\"\nspecifiers = [\"^1\"]\n",
    ));
    assert_eq!(
        matching(&old, &after).unwrap().resolution,
        after[1].resolution
    );
    // 版が変わっても、specifiers が同じものを選ぶ
    let after = entries(concat!(
        "[[tools.node]]\nversion = \"2.0.0\"\nbackend = \"core:node\"\nspecifiers = [\"^2\"]\n",
        "[[tools.node]]\nversion = \"1.5.0\"\nbackend = \"core:node\"\nspecifiers = [\"^1\"]\n",
    ));
    assert_eq!(
        matching(&old, &after).unwrap().resolution,
        after[1].resolution
    );
    // 候補が 1 つなら、版も specifiers も違ってよい
    let after =
        entries("[[tools.node]]\nversion = \"3.0.0\"\nbackend = \"core:node\"\noptions = {}\n");
    assert_eq!(
        matching(&old, &after).unwrap().resolution,
        after[0].resolution
    );
    // 識別の項目が違うもの (別の backend) は候補にならない
    let after = entries(
        "[[tools.node]]\nversion = \"1.0.0\"\nbackend = \"aqua:nodejs/node\"\nspecifiers = [\"^1\"]\n",
    );
    assert!(matching(&old, &after).is_err());
    // 候補が複数あって決められなければエラー
    let after = entries(concat!(
        "[[tools.node]]\nversion = \"2.0.0\"\nbackend = \"core:node\"\n",
        "[[tools.node]]\nversion = \"3.0.0\"\nbackend = \"core:node\"\n",
    ));
    assert_eq!(
        matching(&old, &after).map(|_| ()),
        Err("Cannot unambiguously preserve mise lock platforms for node".to_string())
    );
}

/// install の後のエントリが、前のエントリの platform をすべて、前にあった url と checksum ごと持つかを見る。
/// 増えた platform は問わない。
#[test]
fn covers_requires_every_platform_with_the_metadata_it_had() {
    let required = entries(concat!(
        "[[tools.node]]\nversion = \"1.0.0\"\n",
        "\"platforms.linux-x64\" = { url = \"u\", checksum = \"c\" }\n",
        "\"platforms.macos-arm64\" = {}\n",
    ))
    .remove(0);
    let actual = |platforms: &str| {
        entries(&format!("[[tools.node]]\nversion = \"2.0.0\"\n{platforms}")).remove(0)
    };
    assert!(covers(
        &actual(concat!(
            "\"platforms.linux-x64\" = { url = \"u2\", checksum = \"c2\" }\n",
            "\"platforms.macos-arm64\" = { url = \"u3\" }\n",
            "\"platforms.windows-x64\" = {}\n",
        )),
        &required
    ));
    // checksum を失った
    assert!(!covers(
        &actual(concat!(
            "\"platforms.linux-x64\" = { url = \"u2\" }\n",
            "\"platforms.macos-arm64\" = {}\n",
        )),
        &required
    ));
    // platform を失った
    assert!(!covers(
        &actual("\"platforms.linux-x64\" = { url = \"u2\", checksum = \"c2\" }\n"),
        &required
    ));
}
