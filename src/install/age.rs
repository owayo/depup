//! install と lock refresh に共通する、最終 lock の age 検証と報告。
use crate::cargo_rollback::audit::{LockAgeAuditResult, RemainingAge};
use crate::cargo_rollback::report::lock_age_report_lines;
use crate::domain::{Language, LockAgeFailure, LockAgePackageFailure};
use crate::manifest::RegistryLockEntries;
use crate::node_lock::NodeLockAuditResult;
use crate::progress::Progress;
use colored::Colorize;
use std::path::Path;

/// 復元理由に版を何件まで列挙し、残りを件数でまとめるか。
const NAMED_VERSIONS: usize = 5;

pub(super) fn age_problem(directory: &Path, language: Language, problem: String) -> LockAgeFailure {
    LockAgeFailure {
        directory: directory.into(),
        language,
        packages: Vec::new(),
        problems: vec![problem],
        unchecked: 0,
    }
}

pub(super) fn rust_age_failure(
    directory: &Path,
    audit: &LockAgeAuditResult,
    ownership_problem: Option<String>,
) -> Option<LockAgeFailure> {
    (audit.has_unresolved() || ownership_problem.is_some()).then(|| LockAgeFailure {
        directory: directory.into(),
        language: Language::Rust,
        packages: audit
            .remaining
            .iter()
            .map(|((name, version), age)| LockAgePackageFailure {
                name: name.clone(),
                version: version.clone(),
                reason: rust_age_reason(*age).into(),
            })
            .collect(),
        problems: audit
            .problems
            .iter()
            .cloned()
            .chain(ownership_problem)
            .collect(),
        unchecked: audit.unchecked,
    })
}

pub(super) fn node_age_failure(directory: &Path, audit: &NodeLockAuditResult) -> LockAgeFailure {
    LockAgeFailure {
        directory: directory.into(),
        language: Language::Node,
        packages: audit
            .violations
            .iter()
            .map(|item| LockAgePackageFailure {
                name: item.name.clone(),
                version: item.version.clone(),
                reason: format!(
                    "newer than --age: published {}, cutoff {}",
                    item.released_at.to_rfc3339(),
                    item.cutoff.to_rfc3339()
                ),
            })
            .chain(audit.unverified.iter().map(|item| LockAgePackageFailure {
                name: item.name.clone(),
                version: item.version.clone(),
                reason: item.reason.clone(),
            }))
            .collect(),
        problems: audit
            .errors
            .iter()
            .chain(&audit.unsupported)
            .cloned()
            .collect(),
        unchecked: 0,
    }
}

fn rust_age_reason(age: RemainingAge) -> &'static str {
    match age {
        RemainingAge::TooNew => "newer than --age",
        RemainingAge::Unverified => "release date not verified",
    }
}

/// 最終の lockfile に基づき、単位を実行前の状態へ戻す理由を判定する。
pub(super) fn rust_restore_reason(
    audit: &LockAgeAuditResult,
    baseline: &RegistryLockEntries,
) -> Option<String> {
    let added: Vec<String> = audit
        .remaining
        .iter()
        .filter(|((name, version), _)| {
            !baseline
                .get(name)
                .is_some_and(|versions| versions.contains(version))
        })
        .map(|((name, version), age)| format!("{name} {version} ({})", rust_age_reason(*age)))
        .collect();
    if !added.is_empty() {
        return Some(format!(
            "Cargo.lock keeps versions this run added that do not pass --age: {}",
            named(&added)
        ));
    }
    (!audit.problems.is_empty()).then(|| {
        format!(
            "the Cargo.lock --age audit did not finish: {}",
            audit.problems.join("; ")
        )
    })
}

/// 既存の版の違反も含め、監査の結果を通常出力で知らせる。
pub(super) fn print_rust_audit(
    directory: &Path,
    audit: &LockAgeAuditResult,
    verbose: bool,
    progress: &Progress,
) {
    let lines = lock_age_report_lines(directory, audit, verbose);
    if !lines.is_empty() {
        progress.suspend(|| {
            for line in &lines {
                if line.warning {
                    eprintln!("{}", line.text.yellow());
                } else {
                    eprintln!("{}", line.text);
                }
            }
        });
    }
}

pub(super) fn named(items: &[String]) -> String {
    match items.len() {
        count if count > NAMED_VERSIONS => format!(
            "{}, and {} more",
            items[..NAMED_VERSIONS].join(", "),
            count - NAMED_VERSIONS
        ),
        _ => items.join(", "),
    }
}
