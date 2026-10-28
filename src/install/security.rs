//! install / refresh が共用する、同じパッケージの全解決版の前後比較。
use super::*;
use crate::osv::RiskTradeoff;
use crate::progress::Progress;
use futures::{StreamExt, stream};
use std::future::Future;

pub(super) type Notices = Vec<(String, RiskTradeoff)>;

pub(super) async fn check_changes<F, Fut>(
    before: &BTreeSet<(String, String)>,
    after: &BTreeSet<(String, String)>,
    progress: &Progress,
    check: F,
) -> Result<Notices, String>
where
    F: Fn(String, Vec<String>, Vec<String>) -> Fut,
    Fut: Future<Output = Result<Option<RiskTradeoff>, String>>,
{
    // 追加版だけでなく、消えた版と共存する版も含める。1 件の修正を複数版で使い回さない。
    let names: BTreeSet<_> = after
        .difference(before)
        .map(|(name, _)| name.clone())
        .collect();
    if let Some(bar) = progress.bar() {
        bar.set_position(0);
        bar.set_length(names.len() as u64);
    }
    let requests = stream::iter(names)
        .map(|name| {
            let before: Vec<_> = before
                .iter()
                .filter(|(package, _)| package == &name)
                .map(|(_, version)| version.clone())
                .collect();
            let after: Vec<_> = after
                .iter()
                .filter(|(package, _)| package == &name)
                .map(|(_, version)| version.clone())
                .collect();
            let future = check(name.clone(), before, after);
            let bar = progress.bar();
            async move {
                if let Some(bar) = &bar {
                    bar.set_message(format!("OSV: {name}"));
                }
                let result = future
                    .await
                    .map(|report| report.map(|report| (name, report)));
                if let Some(bar) = &bar {
                    bar.inc(1);
                }
                result
            }
        })
        .buffer_unordered(8);
    tokio::time::timeout(std::time::Duration::from_secs(180), async {
        tokio::pin!(requests);
        let mut notices = Vec::new();
        while let Some(result) = requests.next().await {
            if let Some(notice) = result? {
                notices.push(notice);
            }
        }
        notices.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(notices)
    })
    .await
    .map_err(|_| "lockfile OSV verification timed out".to_string())?
}

pub(super) fn print_notices(notices: Notices, result: &mut OrchestratorResult) {
    for (package, report) in notices {
        let message = report.describe(&package);
        eprintln!("  ⚠ {message}");
        result
            .errors
            .push(crate::orchestrator::OrchestratorError::OsvWarning { package, message });
    }
}

pub(super) fn rust_packages(entries: &RegistryLockEntries) -> BTreeSet<(String, String)> {
    entries
        .iter()
        .flat_map(|(name, versions)| {
            versions
                .iter()
                .map(|version| (name.clone(), version.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osv::{Advisory, Assessment, OsvChecker, Severity};

    #[tokio::test]
    async fn all_coexisting_versions_share_one_fix_budget() {
        let advisory = |id: &str| Advisory {
            id: id.into(),
            aliases: Vec::new(),
            severity: Severity::Medium,
        };
        let checker = OsvChecker::mock_advisories(&[
            ("1.0.0", Ok(vec![advisory("OLD")])),
            ("1.1.0", Ok(vec![advisory("NEW-A")])),
            ("2.0.0", Ok(vec![advisory("NEW-B")])),
        ]);
        for (versions, permitted) in [
            (vec!["1.1.0"], true),
            (vec!["1.1.0", "2.0.0"], false),
            (vec!["1.0.0", "1.1.0"], false),
        ] {
            let before = BTreeSet::from([("example".into(), "1.0.0".into())]);
            let after = versions
                .into_iter()
                .map(|version| ("example".into(), version.into()))
                .collect();
            let checker = &checker;
            let result = check_changes(
                &before,
                &after,
                &Progress::disabled(),
                |name, before, after| async move {
                    match checker
                        .assess_versions("crates.io", &name, &before, &after, true)
                        .await?
                    {
                        Assessment::Safe => Ok(None),
                        Assessment::Tradeoff(report) => Ok(Some(report)),
                        Assessment::Rejected { .. } => {
                            Err("OSV policy rejected the resolution".into())
                        }
                    }
                },
            )
            .await;
            assert_eq!(result.is_ok(), permitted);
        }
    }
}
