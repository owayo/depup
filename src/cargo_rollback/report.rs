//! install 後の Cargo.lock と、画面に出した更新先の版の突き合わせ。
//!
//! depup が表示する更新先は Cargo.toml に書いた版だが、`--install` の `cargo update` は
//! その版要求 (`^0.2.128`) を満たす最新版 (`0.2.129`) を lock に入れる。差し戻しで
//! 揃えられなかった場合や `--max-change` / OSV で古い版を選んだ場合は、表示とビルドに
//! 使われる版が食い違うので、その差を利用者に見せるために使う。

use super::audit::{LOCK_AGE_AUDIT_BUDGET, LockAgeAdjustment, LockAgeAuditResult, LockAgeStatus};
use super::series::same_series;
use crate::manifest::RegistryLockEntries;
use std::cmp::Ordering;
use std::path::Path;

/// 画面に出した 1 件の更新
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayedUpdate {
    /// crates.io 上の crate 名 (リネーム依存でも実パッケージ名)
    pub name: String,
    /// 表示した更新前の版
    pub from: String,
    /// 表示した更新先の版
    pub to: String,
}

/// 更新先の版と Cargo.lock に入った版が食い違った 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedVersionMismatch {
    pub name: String,
    pub from: String,
    pub to: String,
    /// Cargo.lock に入った版
    pub locked: String,
}

/// 更新先と同じ semver 系列で Cargo.lock に入っている版を探し、更新先と違えば返す。
///
/// 同名の crate が複数版 lock されていても (`syn 1.x` と `syn 2.x`)、系列で対応付けるので
/// 取り違えない。系列に該当する版が lock に無いものは突き合わせられないので含めない。
/// 比較は build metadata (`+...`) を無視する。
pub fn lock_mismatches(
    updates: &[DisplayedUpdate],
    entries: &RegistryLockEntries,
) -> Vec<LockedVersionMismatch> {
    updates
        .iter()
        .filter_map(|update| {
            let locked = entries
                .get(&update.name)?
                .iter()
                .find(|version| same_series(version, &update.to))?;
            let differs = match (
                semver::Version::parse(locked),
                semver::Version::parse(&update.to),
            ) {
                (Ok(locked), Ok(to)) => locked.cmp_precedence(&to) != Ordering::Equal,
                _ => locked != &update.to,
            };
            differs.then(|| LockedVersionMismatch {
                name: update.name.clone(),
                from: update.from.clone(),
                to: update.to.clone(),
                locked: locked.clone(),
            })
        })
        .collect()
}

/// stderr に出す 1 行
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportLine {
    pub text: String,
    /// `--age` を満たせなかった・確かめられなかったことを伝える行 (黄色で出す)
    pub warning: bool,
}

impl ReportLine {
    fn info(text: String) -> Self {
        Self {
            text,
            warning: false,
        }
    }

    fn warning(text: String) -> Self {
        Self {
            text,
            warning: true,
        }
    }
}

/// 1 つの Cargo.lock の監査結果を、stderr に出す行へ変換する。
///
/// 差し戻せなかった crate の名前・要求版・理由は `--verbose` が無くても必ず出す。`--age` は供給網
/// 対策として使われるので、満たせなかったことを黙って捨てると、差し戻せた分だけが
/// 表示されて `--age` が効いたように見えてしまう。
pub fn lock_age_report_lines(
    dir: &Path,
    audit: &LockAgeAuditResult,
    verbose: bool,
) -> Vec<ReportLine> {
    let mut lines = Vec::new();
    let dir = dir.display();

    // 元の Cargo.lock を戻せなかった等、差し戻せなかったことより深刻な問題は必ず出す
    for problem in &audit.problems {
        lines.push(ReportLine::warning(format!("  {dir} — {problem}")));
    }

    if audit.unchecked > 0 {
        lines.push(ReportLine::warning(format!(
            "  {dir} — age audit stopped after {}s; {} crate(s) left unchecked",
            LOCK_AGE_AUDIT_BUDGET.as_secs(),
            audit.unchecked
        )));
    }

    if audit.adjustments.is_empty() {
        // 予算切れで未検証が残っている場合は「全て age 内」とは言い切れない
        // (直前に未検証件数を警告済み)
        if verbose && audit.unchecked == 0 && audit.problems.is_empty() {
            lines.push(ReportLine::info(format!(
                "  {dir} — all registry crates in Cargo.lock are within --age"
            )));
        }
        return lines;
    }

    let rolled_back: Vec<&LockAgeAdjustment> = audit
        .adjustments
        .iter()
        .filter(|a| a.status.is_resolved())
        .collect();
    let unverified: Vec<&LockAgeAdjustment> = audit
        .adjustments
        .iter()
        .filter(|a| a.status.is_unverified())
        .collect();
    let restored: Vec<&LockAgeAdjustment> = audit
        .adjustments
        .iter()
        .filter(|a| a.status.is_restored())
        .collect();
    let remaining: Vec<&LockAgeAdjustment> = audit
        .adjustments
        .iter()
        .filter(|a| !a.status.is_resolved() && !a.status.is_restored() && !a.status.is_unverified())
        .collect();

    if !rolled_back.is_empty() {
        lines.push(ReportLine::info(format!(
            "  {dir} — {} crate(s) rolled back to satisfy --age:",
            rolled_back.len()
        )));
        for adj in &rolled_back {
            let to = match adj.status {
                LockAgeStatus::Removed => "removed",
                _ => adj.to.as_deref().unwrap_or("?"),
            };
            lines.push(ReportLine::info(format!(
                "    {} {} → {}",
                adj.name, adj.from, to
            )));
        }
    }

    // install 前の版へ戻しただけで、その版も期間を満たさないもの。「satisfy --age」と
    // 一緒に並べると、期間を満たしたと読めてしまう
    if !restored.is_empty() {
        lines.push(ReportLine::warning(format!(
            "  {dir} — {} crate(s) returned to the version locked before the install, which is also newer than --age:",
            restored.len()
        )));
        for adj in &restored {
            lines.push(ReportLine::warning(format!(
                "    {} {} → {}",
                adj.name,
                adj.from,
                adj.to.as_deref().unwrap_or("?")
            )));
        }
    }

    if !remaining.is_empty() {
        lines.push(ReportLine::warning(format!(
            "  {dir} — {} crate(s) could not be rolled back to satisfy --age:",
            remaining.len()
        )));
        for adj in &remaining {
            let requested = adj
                .target
                .as_ref()
                .map(|version| format!("; requested: {version}"))
                .unwrap_or_default();
            lines.push(ReportLine::warning(format!(
                "    {} ({}{}): {}",
                adj.name,
                adj.from,
                requested,
                lock_age_status_detail(&adj.status)
            )));
        }
    }

    if !unverified.is_empty() {
        lines.push(ReportLine::warning(format!(
            "  {dir} — {} crate(s) could not be checked against --age:",
            unverified.len()
        )));
        for adj in &unverified {
            lines.push(ReportLine::warning(format!(
                "    {} ({}): {}",
                adj.name,
                adj.from,
                lock_age_status_detail(&adj.status)
            )));
        }
    }

    lines
}

/// 差し戻せなかった理由の説明
fn lock_age_status_detail(status: &LockAgeStatus) -> String {
    match status {
        LockAgeStatus::Downgraded => "rolled back".to_string(),
        LockAgeStatus::Removed => "removed from Cargo.lock".to_string(),
        LockAgeStatus::Restored => {
            "returned to the version locked before the install (also newer than --age)".to_string()
        }
        LockAgeStatus::NoOlderCandidate => "no older version satisfies --age".to_string(),
        LockAgeStatus::UpdateCommandFailed(msg) => format!("cargo update failed: {msg}"),
        LockAgeStatus::BlockedByManifest(requirement) => format!(
            "Cargo.toml requires `{requirement}`, which excludes every version that satisfies --age"
        ),
        LockAgeStatus::NotAttempted(reason) => format!("not attempted: {reason}"),
        LockAgeStatus::ReleaseDateUnavailable => "release date unavailable".to_string(),
    }
}

/// 表示と lock の食い違いを stderr に出す行へ変換する
pub fn lock_mismatch_lines(
    manifest_path: &Path,
    mismatches: &[LockedVersionMismatch],
) -> Vec<String> {
    if mismatches.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "  {} — {} update(s) locked at a different version than shown:",
        manifest_path.display(),
        mismatches.len()
    )];
    for mismatch in mismatches {
        lines.push(format!(
            "    {} {} → {} (locked: {})",
            mismatch.name, mismatch.from, mismatch.to, mismatch.locked
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(name: &str, from: &str, to: &str) -> DisplayedUpdate {
        DisplayedUpdate {
            name: name.to_string(),
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    fn entries(pairs: &[(&str, &[&str])]) -> RegistryLockEntries {
        pairs
            .iter()
            .map(|(name, versions)| {
                (
                    (*name).to_string(),
                    versions.iter().map(|v| (*v).to_string()).collect(),
                )
            })
            .collect()
    }

    /// 表示した更新先より新しい版が lock に入ったら注記の対象
    #[test]
    fn test_lock_mismatches_reports_newer_locked_version() {
        let mismatches = lock_mismatches(
            &[update("wasm-bindgen", "0.2.127", "0.2.128")],
            &entries(&[("wasm-bindgen", &["0.2.129"])]),
        );
        assert_eq!(
            mismatches,
            vec![LockedVersionMismatch {
                name: "wasm-bindgen".to_string(),
                from: "0.2.127".to_string(),
                to: "0.2.128".to_string(),
                locked: "0.2.129".to_string(),
            }]
        );
    }

    /// 一致していれば何も出さない
    #[test]
    fn test_lock_mismatches_ignores_matching_version() {
        let mismatches = lock_mismatches(
            &[update("serde", "1.0.200", "1.0.210")],
            &entries(&[("serde", &["1.0.210"])]),
        );
        assert!(mismatches.is_empty());
    }

    /// 同名の別系列の版とは突き合わせない
    #[test]
    fn test_lock_mismatches_matches_by_series() {
        let mismatches = lock_mismatches(
            &[update("syn", "2.0.80", "2.0.90")],
            &entries(&[("syn", &["1.0.109", "2.0.90"])]),
        );
        assert!(mismatches.is_empty());

        let mismatches = lock_mismatches(
            &[update("syn", "2.0.80", "2.0.90")],
            &entries(&[("syn", &["1.0.109", "2.0.95"])]),
        );
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].locked, "2.0.95");
    }

    /// lock に無い crate (その系列の版が無い) は突き合わせられないので含めない
    #[test]
    fn test_lock_mismatches_skips_crates_missing_from_lock() {
        let mismatches = lock_mismatches(
            &[update("tokio", "1.40.0", "1.41.0")],
            &entries(&[("tokio", &["0.2.25"])]),
        );
        assert!(mismatches.is_empty());
    }

    /// build metadata の差だけでは食い違いにしない
    #[test]
    fn test_lock_mismatches_ignores_build_metadata() {
        let mismatches = lock_mismatches(
            &[update("wasi", "0.11.0", "0.11.1+wasi-snapshot-preview1")],
            &entries(&[("wasi", &["0.11.1"])]),
        );
        assert!(mismatches.is_empty());
    }
    fn adjustment(
        name: &str,
        from: &str,
        to: Option<&str>,
        status: LockAgeStatus,
    ) -> LockAgeAdjustment {
        LockAgeAdjustment {
            name: name.to_string(),
            from: from.to_string(),
            to: if status.is_resolved() || status.is_restored() {
                to.map(str::to_string)
            } else {
                None
            },
            target: if status.is_resolved() || status.is_restored() {
                None
            } else {
                to.map(str::to_string)
            },
            status,
        }
    }

    fn texts(lines: &[ReportLine]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn failure_details_include_requested_version_without_verbose() {
        let audit = LockAgeAuditResult {
            adjustments: vec![adjustment(
                "example-udp",
                "0.5.16",
                Some("0.5.15"),
                LockAgeStatus::UpdateCommandFailed("resolver conflict".into()),
            )],
            ..Default::default()
        };
        let lines = lock_age_report_lines(Path::new("."), &audit, false);
        assert!(lines.iter().any(|line| {
            line.text.contains("example-udp")
                && line.text.contains("0.5.16")
                && line.text.contains("requested: 0.5.15")
                && line.text.contains("resolver conflict")
        }));
    }

    /// 差し戻せなかった crate の詳細は `--verbose` が無くても出す。
    /// 見出しは直接依存も含むので `transitive dep(s)` ではなく `crate(s)`
    #[test]
    fn test_lock_age_report_lines_counts_failures_without_verbose() {
        let audit = LockAgeAuditResult {
            adjustments: vec![
                adjustment("cc", "1.5.1", Some("1.4.6"), LockAgeStatus::Downgraded),
                adjustment(
                    "wasm-bindgen",
                    "0.2.129",
                    None,
                    LockAgeStatus::UpdateCommandFailed("conflict".to_string()),
                ),
                adjustment("web-sys", "0.3.106", None, LockAgeStatus::NoOlderCandidate),
            ],
            unchecked: 0,
            problems: Vec::new(),
        };

        let lines = lock_age_report_lines(Path::new("./wasm"), &audit, false);

        assert_eq!(
            texts(&lines),
            vec![
                "  ./wasm — 1 crate(s) rolled back to satisfy --age:",
                "    cc 1.5.1 → 1.4.6",
                "  ./wasm — 2 crate(s) could not be rolled back to satisfy --age:",
                "    wasm-bindgen (0.2.129): cargo update failed: conflict",
                "    web-sys (0.3.106): no older version satisfies --age",
            ]
        );
        assert!(!lines[0].warning);
        assert!(lines[2].warning);
    }

    /// `--verbose` では差し戻せなかった理由を 1 件ずつ出す
    #[test]
    fn test_lock_age_report_lines_lists_reasons_with_verbose() {
        let audit = LockAgeAuditResult {
            adjustments: vec![
                adjustment(
                    "foo",
                    "2.1.3",
                    None,
                    LockAgeStatus::BlockedByManifest("^2.1.3".to_string()),
                ),
                adjustment(
                    "bar",
                    "1.0.2",
                    None,
                    LockAgeStatus::NotAttempted("audit time budget ran out".to_string()),
                ),
            ],
            unchecked: 0,
            problems: Vec::new(),
        };

        let lines = lock_age_report_lines(Path::new("."), &audit, true);

        assert_eq!(
            texts(&lines),
            vec![
                "  . — 2 crate(s) could not be rolled back to satisfy --age:",
                "    foo (2.1.3): Cargo.toml requires `^2.1.3`, which excludes every version that satisfies --age",
                "    bar (1.0.2): not attempted: audit time budget ran out",
            ]
        );
    }

    /// 解き直しで lock から外れた crate も違反が解消したものとして数える
    #[test]
    fn test_lock_age_report_lines_shows_removed_crates_as_rolled_back() {
        let audit = LockAgeAuditResult {
            adjustments: vec![adjustment("tokio", "1.53.1", None, LockAgeStatus::Removed)],
            unchecked: 0,
            problems: Vec::new(),
        };

        let lines = lock_age_report_lines(Path::new("."), &audit, false);

        assert_eq!(
            texts(&lines),
            vec![
                "  . — 1 crate(s) rolled back to satisfy --age:",
                "    tokio 1.53.1 → removed",
            ]
        );
    }

    /// 公開日を取れなかった crate は「差し戻せなかった」とは分けて数える
    #[test]
    fn test_lock_age_report_lines_separates_unverified_crates() {
        let audit = LockAgeAuditResult {
            adjustments: vec![adjustment(
                "private-crate",
                "0.1.0",
                None,
                LockAgeStatus::ReleaseDateUnavailable,
            )],
            unchecked: 3,
            problems: Vec::new(),
        };

        let lines = lock_age_report_lines(Path::new("."), &audit, false);

        assert_eq!(
            texts(&lines),
            vec![
                "  . — age audit stopped after 180s; 3 crate(s) left unchecked",
                "  . — 1 crate(s) could not be checked against --age:",
                "    private-crate (0.1.0): release date unavailable",
            ]
        );
        assert!(lines.iter().all(|line| line.warning));
    }

    /// 何も変えずに全部 age 内なら、`--verbose` のときだけその旨を出す
    #[test]
    fn test_lock_age_report_lines_quiet_when_everything_is_within_age() {
        let audit = LockAgeAuditResult::default();
        assert!(lock_age_report_lines(Path::new("."), &audit, false).is_empty());
        assert_eq!(
            texts(&lock_age_report_lines(Path::new("."), &audit, true)),
            vec!["  . — all registry crates in Cargo.lock are within --age"]
        );
    }

    /// install 前の版へ戻しただけで、その版も期間内のものは「satisfy --age」と分けて出す
    #[test]
    fn test_lock_age_report_lines_separates_restored_crates() {
        let audit = LockAgeAuditResult {
            adjustments: vec![
                adjustment("cc", "1.5.1", Some("1.4.6"), LockAgeStatus::Downgraded),
                adjustment("foo", "1.50.1", Some("1.50.0"), LockAgeStatus::Restored),
            ],
            unchecked: 0,
            problems: Vec::new(),
        };

        let lines = lock_age_report_lines(Path::new("."), &audit, false);

        assert_eq!(
            texts(&lines),
            vec![
                "  . — 1 crate(s) rolled back to satisfy --age:",
                "    cc 1.5.1 → 1.4.6",
                "  . — 1 crate(s) returned to the version locked before the install, which is also newer than --age:",
                "    foo 1.50.1 → 1.50.0",
            ]
        );
        assert!(lines[2].warning);
    }

    /// 元の Cargo.lock を戻せなかった等の問題は `--verbose` が無くても必ず出す
    #[test]
    fn test_lock_age_report_lines_always_shows_problems() {
        let audit = LockAgeAuditResult {
            adjustments: Vec::new(),
            unchecked: 0,
            problems: vec!["restoring the previous Cargo.lock also failed".to_string()],
        };

        let lines = lock_age_report_lines(Path::new("."), &audit, false);

        assert_eq!(
            texts(&lines),
            vec!["  . — restoring the previous Cargo.lock also failed"]
        );
        assert!(lines[0].warning);
    }

    /// 表示した更新先と lock の版が違うときの注記
    #[test]
    fn test_lock_mismatch_lines_formats_locked_version() {
        let mismatches = vec![LockedVersionMismatch {
            name: "wasm-bindgen".to_string(),
            from: "0.2.127".to_string(),
            to: "0.2.128".to_string(),
            locked: "0.2.129".to_string(),
        }];

        let lines = lock_mismatch_lines(Path::new("./Cargo.toml"), &mismatches);

        assert_eq!(
            lines,
            vec![
                "  ./Cargo.toml — 1 update(s) locked at a different version than shown:",
                "    wasm-bindgen 0.2.127 → 0.2.128 (locked: 0.2.129)",
            ]
        );
        assert!(lock_mismatch_lines(Path::new("./Cargo.toml"), &[]).is_empty());
    }
}
