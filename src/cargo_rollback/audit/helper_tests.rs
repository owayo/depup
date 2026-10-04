use super::*;

#[test]
fn unreadable_or_invalid_lock_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Cargo.lock");
    assert!(read_audit_lock(&path).is_err());
    for content in [
        "",
        "broken {{{",
        "package = 1",
        "[[package]]\nname = 'library'\n",
    ] {
        std::fs::write(&path, content).unwrap();
        assert!(read_audit_lock(&path).is_err(), "{content}");
    }
    std::fs::write(&path, "version = 4\n").unwrap();
    assert!(read_audit_lock(&path).unwrap().1.is_empty());
}

fn version_at(version: &str, days_ago: i64) -> VersionInfo {
    VersionInfo::new(
        version,
        chrono::Utc::now() - chrono::Duration::days(days_ago),
    )
}

fn lock_entries(entries: &[(&str, &[&str])]) -> RegistryLockEntries {
    entries
        .iter()
        .map(|(name, versions)| {
            (
                (*name).to_string(),
                versions.iter().map(|v| (*v).to_string()).collect(),
            )
        })
        .collect()
}

/// install 前と同じバージョンで lock されている依存は監査対象に含めない。
/// crates.io は 1 リクエスト/秒 なので、lock 全体を舐めると数百件 = 数分かかる。
#[test]
fn test_changed_entries_excludes_unchanged() {
    let baseline = lock_entries(&[("serde", &["1.0.200"]), ("tokio", &["1.40.0"])]);
    let current = lock_entries(&[("serde", &["1.0.200"]), ("tokio", &["1.41.0"])]);

    let targets = changed_entries(&current, &baseline);

    assert_eq!(
        targets,
        vec![("tokio".to_string(), vec!["1.41.0".to_string()])]
    );
}

/// install で新しく lock に入った依存は監査対象になる
#[test]
fn test_changed_entries_includes_newly_added() {
    let baseline = lock_entries(&[("serde", &["1.0.200"])]);
    let current = lock_entries(&[("serde", &["1.0.200"]), ("anyhow", &["1.0.90"])]);

    let targets = changed_entries(&current, &baseline);

    assert_eq!(
        targets,
        vec![("anyhow".to_string(), vec!["1.0.90".to_string()])]
    );
}

/// 同名クレートが複数バージョン lock されている場合、新しく増えた版だけを対象にする
#[test]
fn test_changed_entries_picks_only_new_versions_of_same_crate() {
    let baseline = lock_entries(&[("syn", &["1.0.109"])]);
    let current = lock_entries(&[("syn", &["1.0.109", "2.0.90"])]);

    let targets = changed_entries(&current, &baseline);

    assert_eq!(
        targets,
        vec![("syn".to_string(), vec!["2.0.90".to_string()])]
    );
}

/// install 前に Cargo.lock が無かった場合 (baseline が空) は全件が対象
#[test]
fn test_changed_entries_empty_baseline_audits_everything() {
    let baseline = RegistryLockEntries::new();
    let current = lock_entries(&[("serde", &["1.0.200"]), ("tokio", &["1.41.0"])]);

    let targets = changed_entries(&current, &baseline);

    assert_eq!(targets.len(), 2);
}

/// 進捗表示と報告順を安定させるため、対象は名前順で返る
/// (`HashMap` の反復順は非決定的)
#[test]
fn test_changed_entries_is_sorted_by_name() {
    let baseline = RegistryLockEntries::new();
    let current = lock_entries(&[("zerocopy", &["0.7.0"]), ("anyhow", &["1.0.90"])]);

    let targets = changed_entries(&current, &baseline);

    let names: Vec<&str> = targets.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["anyhow", "zerocopy"]);
}

/// lock が install 前後で変わっていなければ監査対象は空 (レジストリ照会ゼロ)
#[test]
fn test_changed_entries_no_changes_is_empty() {
    let baseline = lock_entries(&[("serde", &["1.0.200"])]);
    let current = lock_entries(&[("serde", &["1.0.200"])]);

    assert!(changed_entries(&current, &baseline).is_empty());
}

#[test]
fn test_pick_older_within_age_basic() {
    // age cutoff = 14 日前。現在 lock = 1.5.0 (3 日前) は age 違反。
    // 1.4.9 (30 日前) が最新の「古くて age 内」候補。
    let versions = vec![
        version_at("1.4.0", 100),
        version_at("1.4.5", 60),
        version_at("1.4.9", 30),
        version_at("1.5.0", 3), // age 違反の現在 lock
        version_at("1.6.0", 1), // 新しすぎる
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert_eq!(
        pick_older_within_age(&versions, "1.5.0", cutoff, &Default::default()).as_deref(),
        Some("1.4.9"),
    );
}

#[test]
fn test_pick_older_within_age_returns_none_when_all_newer() {
    // 全候補が age 違反または現バージョン以上
    let versions = vec![version_at("1.5.0", 3), version_at("1.6.0", 1)];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert!(pick_older_within_age(&versions, "1.5.0", cutoff, &Default::default()).is_none());
}

#[test]
fn test_pick_older_within_age_skips_prereleases() {
    // プレリリースは候補から除外
    let versions = vec![
        version_at("1.4.9", 30),
        version_at("1.5.0-beta.1", 60), // プレリリースなので除外
        version_at("1.5.0", 3),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert_eq!(
        pick_older_within_age(&versions, "1.5.0", cutoff, &Default::default()).as_deref(),
        Some("1.4.9"),
    );
}

#[test]
fn test_pick_older_within_age_picks_latest_eligible() {
    // 複数の age 内候補があれば semver 最大を選ぶ
    let versions = vec![
        version_at("1.3.0", 200),
        version_at("1.4.0", 100),
        version_at("1.4.9", 30),
        version_at("2.0.0", 5), // 新しすぎる
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert_eq!(
        pick_older_within_age(&versions, "2.0.0", cutoff, &Default::default()).as_deref(),
        Some("1.4.9"),
    );
}

#[test]
fn test_pick_older_within_age_ignores_same_version() {
    // current と同じバージョンは候補外 (downgrade できない)
    let versions = vec![version_at("1.4.0", 30), version_at("1.5.0", 3)];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert_eq!(
        pick_older_within_age(&versions, "1.5.0", cutoff, &Default::default()).as_deref(),
        Some("1.4.0"),
    );
}

/// judge がこの実行で選んだ版 (画面に出した更新先) を差し戻し先の第一候補にする。
/// judge が OSV で 1.4.9 を退けて 1.4.5 を選んでいれば、差し戻しでも 1.4.9 を選び直さない
#[test]
fn test_rollback_target_prefers_judge_choice() {
    let versions = vec![
        version_at("1.4.5", 60),
        version_at("1.4.9", 30),
        version_at("1.5.0", 3),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    let preferred = vec!["1.4.5".to_string()];
    assert_eq!(
        rollback_target(
            &versions,
            "1.5.0",
            cutoff,
            Some(&preferred),
            None,
            &Default::default()
        )
        .as_deref(),
        Some("1.4.5"),
    );
}

/// judge の版が使えない (期間内・今の版以上・別系列) ときは期間を満たす最新の古い版
#[test]
fn test_rollback_target_falls_back_when_judge_choice_is_unusable() {
    let versions = vec![
        version_at("0.9.0", 200),
        version_at("1.4.9", 30),
        version_at("1.5.0", 3),
        version_at("1.5.1", 1),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    for preferred in [
        // 期間内 (新しすぎる)
        vec!["1.5.1".to_string()],
        // 今の版と同じ
        vec!["1.5.0".to_string()],
        // 別の semver 系列
        vec!["0.9.0".to_string()],
    ] {
        assert_eq!(
            rollback_target(
                &versions,
                "1.5.0",
                cutoff,
                Some(&preferred),
                None,
                &Default::default()
            )
            .as_deref(),
            Some("1.4.9"),
            "preferred = {preferred:?}",
        );
    }
    assert_eq!(
        rollback_target(&versions, "1.5.0", cutoff, None, None, &Default::default()).as_deref(),
        Some("1.4.9"),
    );
}

/// 同じ crate を member ごとに別の版へ更新した場合は、使えるもののうち最大を採る
#[test]
fn test_rollback_target_picks_highest_usable_judge_choice() {
    let versions = vec![
        version_at("1.4.0", 90),
        version_at("1.4.5", 60),
        version_at("1.5.0", 3),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    let preferred = vec!["1.4.0".to_string(), "1.4.5".to_string()];
    assert_eq!(
        rollback_target(
            &versions,
            "1.5.0",
            cutoff,
            Some(&preferred),
            None,
            &Default::default()
        )
        .as_deref(),
        Some("1.4.5"),
    );
}

/// install 前の lock にもっと新しい版があったなら、judge の版 (manifest の古い要求から
/// `--max-change` で選んだ版など) まで下げない。下げると install 前に使えていた API が消える
#[test]
fn test_rollback_target_does_not_go_below_version_locked_before_install() {
    let versions = vec![
        version_at("1.40.5", 300),
        version_at("1.50.0", 90),
        version_at("1.52.0", 30),
        version_at("1.53.1", 3),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    let preferred = vec!["1.40.5".to_string()];
    let before = vec!["1.50.0".to_string()];
    assert_eq!(
        rollback_target(
            &versions,
            "1.53.1",
            cutoff,
            Some(&preferred),
            Some(&before),
            &Default::default()
        )
        .as_deref(),
        Some("1.52.0"),
    );
    // 別系列の install 前の版は下限にしない
    let before = vec!["0.9.0".to_string()];
    assert_eq!(
        rollback_target(
            &versions,
            "1.53.1",
            cutoff,
            Some(&preferred),
            Some(&before),
            &Default::default()
        )
        .as_deref(),
        Some("1.40.5"),
    );
}

fn entries_of(pairs: &[(&str, &[&str])]) -> RegistryLockEntries {
    lock_entries(pairs)
}

/// 「差し戻した」と記録した組を最終の lock と突き合わせ、実態に合わせて直す
#[test]
fn test_adjustment_log_reconciles_with_final_lock() {
    let mut log = AdjustmentLog::default();
    // cargo は成功を返したが lock は変わっていない
    log.record(
        "still",
        "1.0.2",
        Some("1.0.1".into()),
        LockAgeStatus::Downgraded,
    );
    // 後の差し戻しでさらに古い版へ動いた
    log.record(
        "moved",
        "1.0.2",
        Some("1.0.1".into()),
        LockAgeStatus::Downgraded,
    );
    // lock から外れた
    log.record("gone", "1.0.2", None, LockAgeStatus::Removed);
    // 今の版より新しい版に動いている
    log.record(
        "newer",
        "1.0.2",
        Some("1.0.1".into()),
        LockAgeStatus::Downgraded,
    );
    // 差し戻していない記録は触らない
    log.record("young", "0.1.0", None, LockAgeStatus::NoOlderCandidate);

    log.reconcile_with_lock(&entries_of(&[
        ("still", &["1.0.2"]),
        ("moved", &["1.0.0"]),
        ("gone", &["0.9.0"]),
        ("newer", &["1.0.3"]),
        ("young", &["0.1.0"]),
    ]));
    let adjustments = log.into_adjustments();
    let by_name = |name: &str| adjustments.iter().find(|a| a.name == name).unwrap();

    assert!(matches!(
        by_name("still").status,
        LockAgeStatus::UpdateCommandFailed(ref m) if m.contains("still has still 1.0.2")
    ));
    assert_eq!(by_name("still").to, None);
    assert_eq!(by_name("moved").status, LockAgeStatus::Downgraded);
    assert_eq!(by_name("moved").to.as_deref(), Some("1.0.0"));
    assert_eq!(by_name("gone").status, LockAgeStatus::Removed);
    assert!(matches!(
        by_name("newer").status,
        LockAgeStatus::UpdateCommandFailed(ref m) if m.contains("not older than 1.0.2")
    ));
    assert_eq!(by_name("young").status, LockAgeStatus::NoOlderCandidate);
}

/// まとめ解きの指紋は lock の内容と候補の集合の両方で変わり、並び順には左右されない
#[test]
fn test_together_fingerprint_depends_on_lock_and_candidates() {
    let candidate = |name: &str| RollbackCandidate {
        name: name.to_string(),
        current: "1.0.2".to_string(),
        target: "1.0.1".to_string(),
        minimum: None,
    };
    let a_b = [candidate("a"), candidate("b")];
    let b_a = [candidate("b"), candidate("a")];
    let a = [candidate("a")];
    assert_eq!(
        together_fingerprint("lock", &a_b),
        together_fingerprint("lock", &b_a)
    );
    assert_ne!(
        together_fingerprint("lock", &a_b),
        together_fingerprint("lock", &a)
    );
    assert_ne!(
        together_fingerprint("lock", &a_b),
        together_fingerprint("other", &a_b)
    );
}

/// 差し戻しの後の lock で、今の版が外れたかと行き先を調べる
#[test]
fn test_locked_after_rollback() {
    let entries = entries_of(&[("syn", &["1.0.109", "2.0.80"])]);
    assert_eq!(
        locked_after_rollback(&entries, "syn", "2.0.90"),
        Some(Some("2.0.80".to_string()))
    );
    assert_eq!(locked_after_rollback(&entries, "syn", "2.0.80"), None);
    assert_eq!(locked_after_rollback(&entries, "syn", "3.0.1"), Some(None));
    assert_eq!(
        locked_after_rollback(&entries, "tokio", "1.0.0"),
        Some(None)
    );
}

/// 期間を満たす古い版が install 前の版より古い (または無い) なら、install 前の版へ戻す。
/// install 前から期間内の版が入っていた場合でも、depup が入れた変更だけを取り消す
#[test]
fn test_rollback_target_returns_to_version_locked_before_install() {
    let versions = vec![
        version_at("1.49.0", 90),
        version_at("1.50.0", 5),
        version_at("1.50.1", 2),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    let before = vec!["1.50.0".to_string()];
    assert_eq!(
        rollback_target(
            &versions,
            "1.50.1",
            cutoff,
            None,
            Some(&before),
            &Default::default()
        )
        .as_deref(),
        Some("1.50.0"),
    );

    let only_young = vec![version_at("1.50.0", 5), version_at("1.50.1", 2)];
    assert_eq!(
        rollback_target(
            &only_young,
            "1.50.1",
            cutoff,
            None,
            Some(&before),
            &Default::default()
        )
        .as_deref(),
        Some("1.50.0"),
    );
}

#[test]
fn an_unavailable_baseline_does_not_allow_another_semver_series() {
    let versions = vec![version_at("0.7.9", 90), version_at("0.8.4", 2)];
    let before = vec!["0.8.3".to_string()];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert!(
        rollback_target(
            &versions,
            "0.8.4",
            cutoff,
            None,
            Some(&before),
            &Default::default()
        )
        .is_none()
    );
}

#[test]
fn a_new_crate_is_not_rolled_back_across_semver_series() {
    let versions = vec![version_at("0.7.9", 90), version_at("0.8.4", 2)];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert!(rollback_target(&versions, "0.8.4", cutoff, None, None, &Default::default()).is_none());
}

#[test]
fn an_available_baseline_still_prevents_going_below_the_preinstall_version() {
    let versions = vec![
        version_at("0.8.1", 90),
        version_at("0.8.2", 5),
        version_at("0.8.4", 2),
    ];
    let before = vec!["0.8.2".to_string(), "0.8.3".to_string()];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert_eq!(
        rollback_target(
            &versions,
            "0.8.4",
            cutoff,
            None,
            Some(&before),
            &Default::default()
        )
        .as_deref(),
        Some("0.8.2")
    );
}
