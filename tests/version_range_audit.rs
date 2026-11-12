//! バージョン指定を、マニフェストの解析・候補判定・書き戻しの全体で検証する。

use chrono::{TimeZone, Utc};
use depup::domain::{Language, SkipReason, UpdateResult};
use depup::manifest::get_parser;
use depup::update::{UpdateFilter, UpdateJudge, VersionInfo};

#[test]
fn cargo_operator_wildcards_keep_spacing_through_manifest_round_trips() {
    let parser = get_parser(Language::Rust);
    let now = Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap();
    let versions = ["1.2.3", "2.3.4"].map(|version| VersionInfo::new(version, now));
    let judge = UpdateJudge::with_time(UpdateFilter::new(), now);

    for (raw, expected) in [
        ("= 1.*", "= 2.*"),
        ("^  1.x", "^  2.x"),
        ("~ 1.2.X", "~ 2.3.X"),
    ] {
        assert!(semver::VersionReq::parse(raw).is_ok(), "{raw}");
        let content = format!("[dependencies]\nexample = '{raw}'\n");
        let dependencies = parser.parse(&content).unwrap();
        assert_eq!(dependencies.len(), 1, "{raw}");
        let UpdateResult::Update { new_version, .. } = judge.judge(&dependencies[0], &versions)
        else {
            panic!("空白付きワイルドカードも更新すること: {raw}");
        };
        let updated = parser
            .update_version(&content, "example", &new_version)
            .unwrap();
        assert_eq!(updated, format!("[dependencies]\nexample = '{expected}'\n"));
        let reparsed = parser.parse(&updated).unwrap();
        assert_eq!(reparsed.len(), 1);
        assert!(matches!(
            judge.judge(&reparsed[0], &versions),
            UpdateResult::Skip {
                reason: SkipReason::AlreadyLatest,
                ..
            }
        ));
    }

    // Cargo が演算子の後に許可するのは半角スペース。ほかの空白へ広げない。
    for raw in ["=\t1.*", "^\u{2003}1.x", "~ 1.x.3"] {
        assert!(semver::VersionReq::parse(raw).is_err(), "{raw}");
        let content = format!("[dependencies]\nexample = '{raw}'\n");
        assert!(parser.parse(&content).unwrap().is_empty(), "{raw}");
    }
}

#[test]
fn qualified_range_limits_survive_manifest_round_trips() {
    let now = Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap();
    let cases = [
        (
            Language::Python,
            "[project]\ndependencies = ['example>=1.0rc1,<1.0rc3']\n",
            "[project]\ndependencies = ['example>=1.0rc2,<1.0rc3']\n",
            ["1.0rc1", "1.0rc2", "1.0rc4"],
        ),
        (
            Language::Php,
            "{\"require\":{\"example/package\":\">=1.0.0 <=1.0.0p2\"}}\n",
            "{\"require\":{\"example/package\":\">=1.0.0p2 <=1.0.0p2\"}}\n",
            ["1.0.0", "1.0.0p2", "1.0.0p3"],
        ),
        (
            Language::Node,
            "{\"dependencies\":{\"example\":\">=1.0.0-rc.1 <1.0.0-rc.3\"}}\n",
            "{\"dependencies\":{\"example\":\">=1.0.0-rc.2 <1.0.0-rc.3\"}}\n",
            ["1.0.0-rc.1", "1.0.0-rc.2", "1.0.0-rc.4"],
        ),
        (
            Language::Rust,
            "[dependencies]\nexample = '>=1.0.0-rc.1, <1.0.0-rc.3'\n",
            "[dependencies]\nexample = '>=1.0.0-rc.2, <1.0.0-rc.3'\n",
            ["1.0.0-rc.1", "1.0.0-rc.2", "1.0.0-rc.4"],
        ),
        (
            Language::Ruby,
            "gem 'example', '>= 1.0.rc1', '< 1.0.rc3'\n",
            "gem 'example', '>= 1.0.rc2', '< 1.0.rc3'\n",
            ["1.0.rc1", "1.0.rc2", "1.0.rc4"],
        ),
        (
            Language::Swift,
            ".package(url: \"https://github.com/example/package.git\", \"1.0.0-rc.1\"..<\"1.0.0-rc.3\")\n",
            ".package(url: \"https://github.com/example/package.git\", \"1.0.0-rc.2\"..<\"1.0.0-rc.3\")\n",
            ["1.0.0-rc.1", "1.0.0-rc.2", "1.0.0-rc.4"],
        ),
        (
            Language::Ruby,
            "gem 'example', '>= 1.0.a', '<= 1.0.x'\n",
            "gem 'example', '>= 1.0.w', '<= 1.0.x'\n",
            ["1.0.a", "1.0.w", "1.0"],
        ),
    ];

    for (language, content, expected, available) in cases {
        let parser = get_parser(language);
        let dependencies = parser.parse(content).unwrap();
        assert_eq!(dependencies.len(), 1, "{language:?}");
        let versions: Vec<_> = available
            .into_iter()
            .map(|version| VersionInfo::new(version, now))
            .collect();
        let judge = UpdateJudge::with_time(UpdateFilter::new(), now);
        let UpdateResult::Update { new_version, .. } = judge.judge(&dependencies[0], &versions)
        else {
            panic!("範囲内の新しい版を選ぶこと: {language:?}");
        };
        let updated = parser
            .update_version(content, dependencies[0].manifest_name(), &new_version)
            .unwrap();
        assert_eq!(updated, expected, "{language:?}");

        // 書き戻した上限を再解析しても、範囲外の版を新たに選んではならない。
        let reparsed = parser.parse(&updated).unwrap();
        assert_eq!(reparsed.len(), 1);
        assert!(
            matches!(
                judge.judge(&reparsed[0], &versions),
                UpdateResult::Skip {
                    reason: SkipReason::AlreadyLatest,
                    ..
                }
            ),
            "{language:?}"
        );
    }
}
