use super::*;

fn advisory(id: &str, severity: Severity) -> Advisory {
    Advisory {
        id: id.into(),
        aliases: Vec::new(),
        severity,
    }
}

#[test]
fn severity_tradeoff_matches_the_default_policy_examples() {
    for (fixed, introduced, permitted) in [
        (Severity::Critical, Severity::Medium, true),
        (Severity::Critical, Severity::Low, true),
        (Severity::High, Severity::Medium, true),
        (Severity::Medium, Severity::Medium, true),
        (Severity::Medium, Severity::Low, true),
        (Severity::Low, Severity::Low, true),
        (Severity::Low, Severity::Medium, false),
        (Severity::Low, Severity::Critical, false),
        (Severity::Critical, Severity::High, false),
        (Severity::Unknown, Severity::Low, false),
    ] {
        assert_eq!(
            tradeoff(&[advisory("OLD", fixed)], &[advisory("NEW", introduced)]).is_some(),
            permitted,
            "{fixed:?} -> {introduced:?}"
        );
    }
    assert!(tradeoff(&[], &[advisory("NEW", Severity::Low)]).is_none());
    assert!(
        tradeoff(
            &[advisory("SAME", Severity::Medium)],
            &[advisory("SAME", Severity::Medium)]
        )
        .is_none()
    );
}

#[test]
fn low_fixes_cannot_pay_for_medium_issues() {
    let mut before = vec![advisory("OLD-MEDIUM", Severity::Medium)];
    before.extend((0..100).map(|n| advisory(&format!("OLD-LOW-{n}"), Severity::Low)));
    let after = vec![
        advisory("NEW-MEDIUM-A", Severity::Medium),
        advisory("NEW-MEDIUM-B", Severity::Medium),
    ];
    assert!(tradeoff(&before, &after).is_none());
    let before = vec![advisory("OLD-CRITICAL", Severity::Critical)];
    assert!(tradeoff(&before, &after).is_none());
}

#[test]
fn fixed_medium_budget_limits_new_low_advisories() {
    let before = vec![
        advisory("MEDIUM-A", Severity::Medium),
        advisory("HIGH-B", Severity::High),
        advisory("RETAINED", Severity::Medium),
    ];
    let mut after = vec![
        advisory("RETAINED", Severity::Medium),
        advisory("LOW-A", Severity::Low),
        advisory("LOW-B", Severity::Low),
    ];
    let result = tradeoff(&before, &after).unwrap();
    assert_eq!(result.fixed.len(), 2);
    assert_eq!(result.introduced.len(), 2);
    assert_eq!(result.retained.len(), 1);
    after.push(advisory("LOW-C", Severity::Low));
    assert!(tradeoff(&before, &after).is_none());
}

#[test]
fn new_high_critical_unknown_and_malware_are_never_offset() {
    let before = vec![advisory("FIXED", Severity::Critical)];
    for severity in [Severity::High, Severity::Critical, Severity::Unknown] {
        assert!(tradeoff(&before, &[advisory("NEW", severity)]).is_none());
    }
    let mut malware = advisory("LOW", Severity::Low);
    malware.aliases.push("MAL-2026-1".into());
    assert!(tradeoff(&before, &[malware]).is_none());
    assert!(
        tradeoff(
            &[advisory("OLD-LOW", Severity::Low)],
            &[advisory("NEW-MEDIUM", Severity::Medium)]
        )
        .is_none()
    );
}

#[test]
fn aliases_are_transitive_and_never_multiply_the_budget() {
    let mut first = advisory("GHSA-old", Severity::Medium);
    first.aliases.push("CVE-old".into());
    let mut duplicate = advisory("RUSTSEC-old", Severity::Unknown);
    duplicate.aliases.push("CVE-old".into());
    let before = vec![first, duplicate, advisory("RETAINED", Severity::Low)];
    let after = vec![
        advisory("RETAINED", Severity::Low),
        advisory("NEW-A", Severity::Low),
        advisory("NEW-B", Severity::Low),
    ];
    assert!(
        tradeoff(&before, &after).is_none(),
        "two aliases of one fix cannot pay for two new issues"
    );
    let result = tradeoff(&before, &after[..2]).unwrap();
    assert_eq!(result.fixed.len(), 1);
    assert_eq!(result.fixed[0].ids.len(), 3);
}

#[test]
fn retaining_a_fixed_issue_in_another_version_gives_no_budget() {
    let before = vec![advisory("EXISTING", Severity::High)];
    let after = vec![
        advisory("EXISTING", Severity::High),
        advisory("NEW", Severity::Low),
    ];
    assert!(tradeoff(&before, &after).is_none());
}

#[test]
fn malware_alias_known_only_to_the_baseline_still_blocks_the_candidate() {
    let mut retained = advisory("GHSA-retained", Severity::Low);
    retained.aliases.push("MAL-2026-example".into());
    let before = [retained, advisory("FIXED", Severity::Critical)];
    let after = [advisory("GHSA-retained", Severity::Low)];
    assert!(tradeoff(&before, &after).is_none());
}

#[test]
fn known_cvss3_vectors_and_scope_changes_are_scored() {
    for (vector, expected) in [
        ("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:L/I:N/A:N", 5.3),
        ("CVSS:3.1/AV:L/AC:L/PR:H/UI:N/S:U/C:L/I:N/A:N", 2.3),
        ("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H", 9.8),
        ("CVSS:3.0/AV:N/AC:L/PR:L/UI:N/S:C/C:H/I:H/A:H", 9.9),
    ] {
        assert_eq!(cvss3(vector), Some(expected));
    }
    assert_eq!(cvss3("CVSS:4.0/AV:N"), None);
    assert_eq!(cvss3("CVSS:3.1/AV:N/AV:L"), None);
}

#[test]
fn severity_uses_highest_known_assessment_and_unknown_stays_unknown() {
    let ratings = vec![RawSeverity {
        kind: "CVSS_V3".into(),
        score: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:L/I:N/A:N".into(),
    }];
    assert_eq!(severity(Some("LOW"), &ratings), Severity::Medium);
    assert_eq!(severity(Some("HIGH"), &ratings), Severity::High);
    assert_eq!(severity(None, &[]), Severity::Unknown);
}
