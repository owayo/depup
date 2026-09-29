//! 1 件ずつの `cargo update --precise` では戻せない crate を、まとめて解き直す。
//!
//! wasm-bindgen 一族のように互いを `=` で固定し合う crate 群では、
//! `cargo update -p X --precise V` が X の依存元を lock の版に据え置くため、一族の中に
//! 「頂点」(一族内に依存元を持たない crate) が 2 つ以上あると、どの順に 1 件ずつ
//! 戻しても残りの `=` 指定と衝突する。
//!
//! 利用者の Cargo.toml には触れず、workspace の写し ([`ScratchWorkspace`]) に固定用の
//! 別名依存 (`=目標`) を足し、対象を `-p` に並べて一度に解き直す。固定を外してから
//! `cargo update --workspace` で lock を元の manifest と同じ依存の形に整え、元の
//! Cargo.lock を置き換えたら、元の manifest がその lock をそのまま受け入れること
//! (`cargo update --workspace --locked`) を確かめる。受け入れられなければ、置き換える
//! 前の lock に戻す。

use super::graph::{LockGraph, PackageKey};
use super::scratch::{AgePin, CargoCommand, CargoError, ScratchWorkspace};
use super::series::same_series;
use crate::manifest::{RegistryLockEntries, parse_registry_entries, write_manifest};
use crate::update::compare_versions;
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::{Duration, Instant};

/// cargo を 1 回起動するごとに、最低限残っていてほしい時間。これより短いなら起動しない。
///
/// 解き直しは cargo を 3 回 (解き直し・固定の除去・元での検証) 続けて起動し、最後の
/// 検証まで届いて初めて採用できる。元の Cargo.lock を置き換える前にも、検証の分が
/// 残っているかを確かめる (置き換えてから時間切れになると、戻すだけの無駄になる)。
const MIN_COMMAND_SLICE: Duration = Duration::from_secs(5);

/// まとめて解き直す 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackCandidate {
    /// crates.io 上の crate 名
    pub name: String,
    /// Cargo.lock 上の現在の版 (期間を満たさない)
    pub current: String,
    /// 戻したい版 (期間を満たす)
    pub target: String,
    /// これより古くしない版 (install 前の lock にあった同じ系列の版)。範囲で固定するときの
    /// 下端に使い、解き直した結果がこれを割れば失敗にする
    pub minimum: Option<String>,
}

impl RollbackCandidate {
    fn key(&self) -> PackageKey {
        (self.name.clone(), self.current.clone())
    }

    /// 同名の crate が複数版 lock されていても対象を一意にする `name@version` 形式
    fn spec(&self) -> String {
        format!("{}@{}", self.name, self.current)
    }
}

/// まとめ解きの 1 件ごとの結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TogetherOutcome {
    /// 期間を満たす版へ戻った (値は Cargo.lock に入った版)
    Resolved(String),
    /// 解き直した結果、この crate を必要とする依存が無くなり Cargo.lock から外れた
    Removed,
    /// Cargo.toml の版要求が目標の版を許さない (値はその要求)
    BlockedByManifest(String),
    /// まとめて解いても戻せなかった (値は理由)
    Failed(String),
    /// 時間予算が尽きて、まとめ解きを試せなかった・最後まで試せなかった (値は理由)
    NotAttempted(String),
}

/// まとめ解きの結果
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TogetherReport {
    /// 候補ごとの結果 (入力と同じ順)
    pub outcomes: Vec<(RollbackCandidate, TogetherOutcome)>,
    /// 利用者に必ず知らせるべき問題 (元の Cargo.lock を元に戻せなかった等)。
    /// 差し戻せなかったことより深刻なので、`--verbose` の有無に関係なく表示する
    pub problems: Vec<String>,
}

/// 1 回の試行の失敗
enum AttemptError {
    /// この組み合わせでは解けなかった (固定の仕方を変えれば解けうる)
    Unresolved(String),
    /// 続けても意味が無い失敗 (写しの操作に失敗した、作業中に元のファイルが書き換わった、
    /// 元の manifest が解き直した lock を受け入れない)
    Abort(String),
    /// 時間予算が尽きた。cargo の失敗ではないので、別の手を試さずに打ち切る
    OutOfTime(String),
    /// 続けても意味が無いうえ、元の Cargo.lock を元の内容へ戻せなかった
    Broken(String),
}

/// cargo を 1 回起動した結果の失敗
enum RunError {
    /// 時間予算が尽きていて起動しなかった、または上限の時間内に終わらなかった
    OutOfTime(String),
    /// cargo が失敗した (値は stderr などの説明)
    Failed(String),
}

/// `candidates` をまとめて解き直し、元の Cargo.lock (`root/Cargo.lock`) へ反映する。
///
/// lock の依存辺でつながる組 (連結成分) ごとに、大きい組から解く。辺の無い組どうしは
/// 互いの固定に縛られないので、まとめて試す利点が無く、解けない 1 件が一族を巻き添えに
/// するだけになる。組ごとに試す固定の仕方は [`pin_plans`] の順 (全員を `=目標` →
/// 頂点だけ `=` → 全員を系列内で目標以下)。
///
/// 反映した lock は元の manifest が受け入れることを確かめてから確定する。時間予算が
/// 尽きたら残りの組は `NotAttempted` にする。戻り値は `candidates` と同じ順に並ぶ。
pub async fn resolve_together(
    root: &Path,
    candidates: &[RollbackCandidate],
    cargo: &CargoCommand,
    deadline: Instant,
    per_command: Duration,
) -> TogetherReport {
    let mut outcomes: HashMap<PackageKey, TogetherOutcome> = HashMap::new();
    let mut problems: Vec<String> = Vec::new();
    let mut eligible: Vec<RollbackCandidate> = Vec::new();
    for candidate in candidates {
        let key = candidate.key();
        if outcomes.contains_key(&key) || eligible.iter().any(|seen| seen.key() == key) {
            continue;
        }
        // 系列をまたぐ目標は、別名依存で固定しても別系列の版が lock に増えるだけで、
        // 今の版は外れない
        if same_series(&candidate.target, &candidate.current) {
            eligible.push(candidate.clone());
        } else {
            outcomes.insert(
                candidate.key(),
                TogetherOutcome::Failed(format!(
                    "no version older than {} in the same semver series satisfies --age",
                    candidate.current
                )),
            );
        }
    }
    if eligible.is_empty() {
        return finish(candidates, outcomes, problems);
    }

    let Some(timeout) = command_timeout(deadline, per_command) else {
        let outcome = TogetherOutcome::NotAttempted(BUDGET_EXHAUSTED.to_string());
        set_all(&mut outcomes, &eligible, &outcome);
        return finish(candidates, outcomes, problems);
    };
    let mut scratch = match ScratchWorkspace::create(root, cargo, timeout).await {
        Ok(scratch) => scratch,
        Err(e) => {
            let outcome = TogetherOutcome::Failed(format!(
                "could not copy the workspace to re-resolve it: {e}"
            ));
            set_all(&mut outcomes, &eligible, &outcome);
            return finish(candidates, outcomes, problems);
        }
    };

    // Cargo.toml 自身が今の版以上を要求していると、どう解いても目標の版には戻れない。
    // ここで外さないと、解けない 1 件がまとめ解き全体を失敗させる
    eligible.retain(
        |candidate| match blocking_requirement(&scratch, candidate) {
            Some(requirement) => {
                outcomes.insert(
                    candidate.key(),
                    TogetherOutcome::BlockedByManifest(requirement),
                );
                false
            }
            None => true,
        },
    );

    // lock の依存辺でつながる組ごとに、大きい組から解く (同じ大きさなら入力順)。
    // 時間予算が尽きたときに、戻す crate の多い一族を先に済ませておける
    let graph = LockGraph::from_lock_content(scratch.original_lock());
    let keys: Vec<PackageKey> = eligible.iter().map(RollbackCandidate::key).collect();
    let mut groups: Vec<Vec<RollbackCandidate>> = graph
        .components(&keys)
        .into_iter()
        .map(|component| {
            eligible
                .iter()
                .filter(|candidate| component.contains(&candidate.key()))
                .cloned()
                .collect()
        })
        .collect();
    groups.sort_by_key(|group| std::cmp::Reverse(group.len()));

    let mut queue: VecDeque<Vec<RollbackCandidate>> = groups.into();
    while let Some(group) = queue.pop_front() {
        // 前の組を反映した結果、既に今の版から動いた crate は、その結果で確定させる
        // (`-p name@今の版` が lock に無い版を指して cargo が失敗するのを避ける)
        let entries = parse_registry_entries(scratch.original_lock());
        let (group, moved): (Vec<RollbackCandidate>, Vec<RollbackCandidate>) =
            group.into_iter().partition(|candidate| {
                entries
                    .get(&candidate.name)
                    .is_some_and(|versions| versions.contains(&candidate.current))
            });
        for candidate in moved {
            let outcome = resolution_outcome(&candidate, &entries, false)
                .unwrap_or_else(TogetherOutcome::Failed);
            outcomes.insert(candidate.key(), outcome);
        }
        if group.is_empty() {
            continue;
        }
        // 前の組を反映した後は lock が変わっているので、グラフも取り直す
        let graph = LockGraph::from_lock_content(scratch.original_lock());
        let keys: Vec<PackageKey> = group.iter().map(RollbackCandidate::key).collect();

        let mut result = Err(AttemptError::Unresolved(String::new()));
        let mut first_reason: Option<String> = None;
        for plan in pin_plans(&group, &graph.tops(&keys)) {
            result = attempt(
                &mut scratch,
                root,
                cargo,
                &group,
                &plan,
                deadline,
                per_command,
            )
            .await;
            match &result {
                // 後の手でも解けなかったときは、最初 (全員を `=` で固定) の理由の方が
                // 利用者にとって読みやすい (どの `=` 指定が衝突したかが出る)
                Err(AttemptError::Unresolved(reason)) => {
                    first_reason.get_or_insert_with(|| reason.clone());
                }
                // 成功、または続けても意味が無い失敗 (時間切れを含む) なら次の手は試さない
                _ => break,
            }
        }
        if let (Err(AttemptError::Unresolved(reason)), Some(first)) = (&mut result, first_reason) {
            *reason = first;
        }

        match result {
            Ok(resolved) => outcomes.extend(resolved),
            Err(AttemptError::Unresolved(reason)) => {
                set_all(&mut outcomes, &group, &TogetherOutcome::Failed(reason));
            }
            Err(AttemptError::OutOfTime(reason)) => {
                let outcome = TogetherOutcome::NotAttempted(reason);
                set_all(&mut outcomes, &group, &outcome);
                for rest in queue.drain(..) {
                    set_all(&mut outcomes, &rest, &outcome);
                }
                break;
            }
            Err(AttemptError::Abort(reason)) => {
                let outcome = TogetherOutcome::Failed(reason);
                set_all(&mut outcomes, &group, &outcome);
                for rest in queue.drain(..) {
                    set_all(&mut outcomes, &rest, &outcome);
                }
                break;
            }
            Err(AttemptError::Broken(reason)) => {
                problems.push(reason.clone());
                let outcome = TogetherOutcome::Failed(reason);
                set_all(&mut outcomes, &group, &outcome);
                for rest in queue.drain(..) {
                    set_all(&mut outcomes, &rest, &outcome);
                }
                break;
            }
        }
    }

    finish(candidates, outcomes, problems)
}

/// 時間切れで着手できなかったときの理由
const BUDGET_EXHAUSTED: &str = "audit time budget ran out before resolving together";

/// 1 回の解き直しでの固定の仕方
#[derive(Debug, Clone, PartialEq, Eq)]
struct PinPlan {
    /// 写しに足す別名依存
    pins: Vec<AgePin>,
    /// `=` で目標の版に固定した crate (解き直した後、目標の版ちょうどでなければならない)
    exact: Vec<PackageKey>,
}

/// 試す固定の仕方を順に返す。
///
/// 1. 全員を目標の版に `=` で固定する
/// 2. 一族の頂点だけを `=` で固定し、残りは頂点の `=` 指定に従わせる
///    (各 crate で独立に選んだ目標同士が噛み合わない場合に効く)
/// 3. 全員を「同じ系列で目標以下」の範囲で固定する (judge が一族の 1 つだけ古い版を
///    選んだ場合など、目標そのものが揃わない場合に効く)。目標より古い版が選ばれうるが、
///    解き直した lock は監査の次のパスで公開日を確かめ直すので、未確認のまま残らない
fn pin_plans(group: &[RollbackCandidate], tops: &[PackageKey]) -> Vec<PinPlan> {
    let mut plans = vec![PinPlan {
        pins: group.iter().map(exact_pin).collect(),
        exact: group.iter().map(RollbackCandidate::key).collect(),
    }];
    let top_members: Vec<&RollbackCandidate> = group
        .iter()
        .filter(|candidate| tops.contains(&candidate.key()))
        .collect();
    if !top_members.is_empty() && top_members.len() < group.len() {
        plans.push(PinPlan {
            pins: top_members
                .iter()
                .map(|candidate| exact_pin(candidate))
                .collect(),
            exact: top_members
                .iter()
                .map(|candidate| candidate.key())
                .collect(),
        });
    }
    plans.push(PinPlan {
        pins: group.iter().map(at_most_pin).collect(),
        exact: Vec::new(),
    });
    plans
}

/// 目標の版ちょうどに固定する
fn exact_pin(candidate: &RollbackCandidate) -> AgePin {
    AgePin {
        name: candidate.name.clone(),
        requirement: format!("={}", candidate.target),
    }
}

/// 同じ semver 系列の中で目標の版以下に固定する。下端は系列の最初の版か、install 前の
/// lock の版 (`minimum`) の新しい方。`0.0.x` は版ごとに別系列なので `=` にする
fn at_most_pin(candidate: &RollbackCandidate) -> AgePin {
    let floor = match (
        series_floor(&candidate.target),
        candidate.minimum.as_deref(),
    ) {
        (Some(series), Some(minimum))
            if compare_versions(minimum, &series) == Ordering::Greater =>
        {
            Some(minimum.to_string())
        }
        (series, _) => series,
    };
    let requirement = match floor {
        Some(floor) if compare_versions(&floor, &candidate.target) == Ordering::Less => {
            format!(">={floor}, <={}", candidate.target)
        }
        _ => format!("={}", candidate.target),
    };
    AgePin {
        name: candidate.name.clone(),
        requirement,
    }
}

/// semver 系列の最初の版 (`1.4.9` → `1.0.0`、`0.2.128` → `0.2.0`)。`0.0.x` は None
fn series_floor(version: &str) -> Option<String> {
    let version = semver::Version::parse(version).ok()?;
    if version.major > 0 {
        Some(format!("{}.0.0", version.major))
    } else if version.minor > 0 {
        Some(format!("0.{}.0", version.minor))
    } else {
        None
    }
}

/// `group` を `plan` どおりに固定して解き直し、元の Cargo.lock へ反映する
async fn attempt(
    scratch: &mut ScratchWorkspace,
    root: &Path,
    cargo: &CargoCommand,
    group: &[RollbackCandidate],
    plan: &PinPlan,
    deadline: Instant,
    per_command: Duration,
) -> Result<Vec<(PackageKey, TogetherOutcome)>, AttemptError> {
    scratch
        .reset_lock()
        .map_err(|e| AttemptError::Abort(format!("could not reset the copied Cargo.lock: {e}")))?;
    scratch
        .set_pins(&plan.pins)
        .map_err(|e| AttemptError::Abort(format!("could not write the temporary pins: {e}")))?;

    let manifest = scratch.manifest_path().into_os_string();
    let mut update: Vec<OsString> =
        vec!["update".into(), "--manifest-path".into(), manifest.clone()];
    for candidate in group {
        update.push("-p".into());
        update.push(candidate.spec().into());
    }
    run_cargo(cargo, root, &update, deadline, per_command)
        .await
        .map_err(|e| match e {
            RunError::Failed(message) => AttemptError::Unresolved(message),
            RunError::OutOfTime(message) => AttemptError::OutOfTime(message),
        })?;

    // 固定を外して lock を元の manifest と同じ依存の形に整える。`--workspace` は
    // workspace の member だけを解き直すので、固定で決まった版はそのまま残る
    scratch
        .set_pins(&[])
        .map_err(|e| AttemptError::Abort(format!("could not remove the temporary pins: {e}")))?;
    let reconcile: Vec<OsString> = vec![
        "update".into(),
        "--workspace".into(),
        "--manifest-path".into(),
        manifest,
    ];
    run_cargo(cargo, root, &reconcile, deadline, per_command)
        .await
        .map_err(|e| match e {
            RunError::Failed(message) => {
                AttemptError::Unresolved(format!("could not drop the temporary pins: {message}"))
            }
            RunError::OutOfTime(message) => AttemptError::OutOfTime(message),
        })?;

    let resolved_lock = std::fs::read_to_string(scratch.lock_path()).map_err(|e| {
        AttemptError::Abort(format!("could not read the re-resolved Cargo.lock: {e}"))
    })?;
    let resolved =
        check_resolution(group, &plan.exact, &resolved_lock).map_err(AttemptError::Unresolved)?;

    // 作業中に利用者やエディタが元の Cargo.toml / Cargo.lock を書き換えていたら、
    // そちらを優先して手を引く
    if !scratch.originals_unchanged() {
        return Err(AttemptError::Abort(
            "Cargo.toml or Cargo.lock changed while depup was rolling back".to_string(),
        ));
    }
    // 置き換えた後に検証の時間が無いと、戻すだけの無駄になる (理由も取り違える)
    if command_timeout(deadline, per_command).is_none() {
        return Err(AttemptError::OutOfTime(BUDGET_EXHAUSTED.to_string()));
    }
    let lock_path = root.join("Cargo.lock");
    write_manifest(&lock_path, &resolved_lock)
        .map_err(|e| AttemptError::Abort(format!("could not write Cargo.lock: {e}")))?;

    // 元の manifest (`=` の固定を持たない) が、この lock をそのまま受け入れるか
    let verify: Vec<OsString> = vec!["update".into(), "--workspace".into(), "--locked".into()];
    if let Err(e) = run_cargo(cargo, root, &verify, deadline, per_command).await {
        let failure = match e {
            RunError::Failed(message) => AttemptError::Abort(format!(
                "Cargo.toml did not accept the re-resolved Cargo.lock: {message}"
            )),
            RunError::OutOfTime(message) => {
                AttemptError::OutOfTime(format!("checking the re-resolved Cargo.lock {message}"))
            }
        };
        // 置いた lock がそのまま残っているときだけ戻す (その間の他者の書き換えは消さない)。
        // 確認から書き戻しまでの間に他のプロセスが書いた変更までは守れない (cargo 自身の
        // コマンドも Cargo.lock を排他せずに書くので、同じ条件になる)
        let ours = std::fs::read_to_string(&lock_path).is_ok_and(|now| now == resolved_lock);
        if ours && let Err(restore) = write_manifest(&lock_path, scratch.original_lock()) {
            let reason = match failure {
                AttemptError::Abort(reason) | AttemptError::OutOfTime(reason) => reason,
                AttemptError::Unresolved(reason) | AttemptError::Broken(reason) => reason,
            };
            return Err(AttemptError::Broken(format!(
                "{reason}; restoring the previous {} also failed ({restore}); {}",
                lock_path.display(),
                save_backup(scratch.original_lock())
            )));
        }
        return Err(failure);
    }
    scratch.accept_lock(resolved_lock);
    Ok(resolved)
}

/// 元へ戻せなかった Cargo.lock の内容を OS の一時ディレクトリへ退避し、その説明を返す。
/// 退避先は既存のファイルを上書きしない (`create_new`)
fn save_backup(content: &str) -> String {
    use std::io::Write as _;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "depup-Cargo.lock.{}.{nanos}.bak",
        std::process::id()
    ));
    let saved = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut file| file.write_all(content.as_bytes()));
    match saved {
        Ok(()) => format!(
            "the previous content was saved to {} — copy it back to restore",
            path.display()
        ),
        Err(e) => format!("saving the previous content elsewhere also failed ({e})"),
    }
}

/// 解き直した lock で、各 crate が今の版から外れて同じ系列の古い版へ移ったかを確かめる。
/// `=` で固定した crate (`exact`) は目標の版ちょうどでなければならない
fn check_resolution(
    group: &[RollbackCandidate],
    exact: &[PackageKey],
    lock: &str,
) -> Result<Vec<(PackageKey, TogetherOutcome)>, String> {
    let entries = parse_registry_entries(lock);
    group
        .iter()
        .map(|candidate| {
            let outcome =
                resolution_outcome(candidate, &entries, exact.contains(&candidate.key()))?;
            Ok((candidate.key(), outcome))
        })
        .collect()
}

/// lock (`entries`) の上で、1 件が今の版から外れて同じ系列の古い版へ移ったかを判定する。
/// `exact` なら目標の版ちょうどでなければならない
fn resolution_outcome(
    candidate: &RollbackCandidate,
    entries: &RegistryLockEntries,
    exact: bool,
) -> Result<TogetherOutcome, String> {
    let versions = entries
        .get(&candidate.name)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if versions.contains(&candidate.current) {
        return Err(format!(
            "{} {} is still locked after re-resolving",
            candidate.name, candidate.current
        ));
    }
    let in_series: Vec<&String> = versions
        .iter()
        .filter(|version| same_series(version, &candidate.current))
        .collect();
    match in_series.as_slice() {
        [] => Ok(TogetherOutcome::Removed),
        [version] => {
            if compare_versions(version, &candidate.current) != Ordering::Less {
                return Err(format!(
                    "{} moved to {}, which is not older than {}",
                    candidate.name, version, candidate.current
                ));
            }
            if exact && compare_versions(version, &candidate.target) != Ordering::Equal {
                return Err(format!(
                    "{} resolved to {} instead of the pinned {}",
                    candidate.name, version, candidate.target
                ));
            }
            // install 前より古い版まで下げると、それまで使えていた API が消える
            if let Some(minimum) = &candidate.minimum
                && compare_versions(version, minimum) == Ordering::Less
            {
                return Err(format!(
                    "{} resolved to {}, older than {} locked before the install",
                    candidate.name, version, minimum
                ));
            }
            Ok(TogetherOutcome::Resolved((*version).clone()))
        }
        // Cargo は同じ系列の版を 1 つの lock に 2 つ置かないので、ここに来るのは lock が壊れているとき
        _ => Err(format!(
            "{} is locked at several versions of the same series",
            candidate.name
        )),
    }
}

/// workspace の member が Cargo.toml に書いた版要求のうち、今の版を許して目標の版を
/// 許さないもの (= Cargo.toml を変えない限り目標へ戻せない理由)
fn blocking_requirement(
    scratch: &ScratchWorkspace,
    candidate: &RollbackCandidate,
) -> Option<String> {
    let current = semver::Version::parse(&candidate.current).ok()?;
    let target = semver::Version::parse(&candidate.target).ok()?;
    scratch
        .direct_requirements(&candidate.name)
        .into_iter()
        .find(|requirement| {
            semver::VersionReq::parse(requirement)
                .is_ok_and(|req| req.matches(&current) && !req.matches(&target))
        })
}

/// 残り時間から cargo 1 回分の上限を決める。着手する余裕が無ければ None
fn command_timeout(deadline: Instant, per_command: Duration) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (remaining >= MIN_COMMAND_SLICE).then(|| remaining.min(per_command))
}

/// cargo を 1 回起動する。時間予算が尽きていれば起動せず、時間切れは cargo の失敗と
/// 区別して返す
async fn run_cargo(
    cargo: &CargoCommand,
    root: &Path,
    args: &[OsString],
    deadline: Instant,
    per_command: Duration,
) -> Result<String, RunError> {
    let Some(timeout) = command_timeout(deadline, per_command) else {
        return Err(RunError::OutOfTime(BUDGET_EXHAUSTED.to_string()));
    };
    let args: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
    cargo.run(root, &args, timeout).await.map_err(|e| match e {
        CargoError::TimedOut(_) => RunError::OutOfTime(e.to_string()),
        CargoError::Failed(message) => RunError::Failed(message),
    })
}

/// 組の全員に同じ結果を付ける
fn set_all(
    outcomes: &mut HashMap<PackageKey, TogetherOutcome>,
    group: &[RollbackCandidate],
    outcome: &TogetherOutcome,
) {
    for candidate in group {
        outcomes.insert(candidate.key(), outcome.clone());
    }
}

fn finish(
    candidates: &[RollbackCandidate],
    outcomes: HashMap<PackageKey, TogetherOutcome>,
    problems: Vec<String>,
) -> TogetherReport {
    TogetherReport {
        outcomes: in_input_order(candidates, outcomes),
        problems,
    }
}

fn in_input_order(
    candidates: &[RollbackCandidate],
    mut outcomes: HashMap<PackageKey, TogetherOutcome>,
) -> Vec<(RollbackCandidate, TogetherOutcome)> {
    let mut ordered = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        // 同じ組が重複して渡された場合は最初の 1 件だけに結果を付ける
        if let Some(outcome) = outcomes.remove(&candidate.key()) {
            ordered.push((candidate.clone(), outcome));
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str, current: &str, target: &str) -> RollbackCandidate {
        RollbackCandidate {
            name: name.to_string(),
            current: current.to_string(),
            target: target.to_string(),
            minimum: None,
        }
    }

    fn candidate_with_minimum(
        name: &str,
        current: &str,
        target: &str,
        minimum: &str,
    ) -> RollbackCandidate {
        RollbackCandidate {
            minimum: Some(minimum.to_string()),
            ..candidate(name, current, target)
        }
    }

    fn key(name: &str, version: &str) -> PackageKey {
        (name.to_string(), version.to_string())
    }

    fn lock(packages: &[(&str, &str)]) -> String {
        let mut content = String::from("version = 4\n");
        for (name, version) in packages {
            content.push_str(&format!(
                "\n[[package]]\nname = \"{name}\"\nversion = \"{version}\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n"
            ));
        }
        content
    }

    /// 固定した crate が目標の版に、固定していない crate も今より古い同じ系列の版に移れば成功
    #[test]
    fn test_check_resolution_accepts_pinned_and_derived_versions() {
        let group = vec![
            candidate("fam-a", "1.0.2", "1.0.1"),
            candidate("fam-core", "1.0.2", "1.0.1"),
        ];
        let resolved = check_resolution(
            &group,
            &[key("fam-a", "1.0.2")],
            &lock(&[("fam-a", "1.0.1"), ("fam-core", "1.0.0")]),
        )
        .unwrap();
        assert_eq!(
            resolved,
            vec![
                (
                    ("fam-a".to_string(), "1.0.2".to_string()),
                    TogetherOutcome::Resolved("1.0.1".to_string())
                ),
                (
                    ("fam-core".to_string(), "1.0.2".to_string()),
                    TogetherOutcome::Resolved("1.0.0".to_string())
                ),
            ]
        );
    }

    /// 今の版が残っていれば失敗
    #[test]
    fn test_check_resolution_rejects_still_locked_version() {
        let group = vec![candidate("fam-a", "1.0.2", "1.0.1")];
        let err = check_resolution(&group, &[], &lock(&[("fam-a", "1.0.2")])).unwrap_err();
        assert!(err.contains("still locked"), "{err}");
    }

    /// 固定した crate が目標と違う版に落ちたら失敗 (固定が効いていない)
    #[test]
    fn test_check_resolution_rejects_pinned_crate_on_other_version() {
        let group = vec![candidate("fam-a", "1.0.2", "1.0.1")];
        let err = check_resolution(
            &group,
            &[key("fam-a", "1.0.2")],
            &lock(&[("fam-a", "1.0.0")]),
        )
        .unwrap_err();
        assert!(err.contains("instead of the pinned"), "{err}");
    }

    /// 同じ系列の版が lock から消えたら、依存が無くなって外れたものとして扱う
    #[test]
    fn test_check_resolution_reports_removed_crate() {
        let group = vec![candidate("tokio", "1.53.1", "1.53.0")];
        let resolved = check_resolution(&group, &[], &lock(&[("tokio", "0.2.25")])).unwrap();
        assert_eq!(resolved[0].1, TogetherOutcome::Removed);
    }

    /// 同名でも別系列の版 (`syn 1.x` と `syn 2.x`) は取り違えない
    #[test]
    fn test_check_resolution_matches_by_series() {
        let group = vec![candidate("syn", "2.0.90", "2.0.80")];
        let resolved = check_resolution(
            &group,
            &[key("syn", "2.0.90")],
            &lock(&[("syn", "1.0.109"), ("syn", "2.0.80")]),
        )
        .unwrap();
        assert_eq!(
            resolved[0].1,
            TogetherOutcome::Resolved("2.0.80".to_string())
        );
    }

    /// 今より新しい版へ動いたものは差し戻しではないので失敗
    #[test]
    fn test_check_resolution_rejects_newer_version() {
        let group = vec![candidate("fam-a", "1.0.2", "1.0.1")];
        let err = check_resolution(&group, &[], &lock(&[("fam-a", "1.0.3")])).unwrap_err();
        assert!(err.contains("not older than"), "{err}");
    }

    /// 結果は入力順に並び、重複した組には 1 回だけ結果を付ける
    #[test]
    fn test_in_input_order_keeps_order_and_drops_duplicates() {
        let a = candidate("a", "1.0.1", "1.0.0");
        let b = candidate("b", "1.0.1", "1.0.0");
        let mut outcomes = HashMap::new();
        outcomes.insert(b.key(), TogetherOutcome::Removed);
        outcomes.insert(a.key(), TogetherOutcome::Resolved("1.0.0".to_string()));

        let ordered = in_input_order(&[a.clone(), b.clone(), a.clone()], outcomes);

        assert_eq!(
            ordered,
            vec![
                (a, TogetherOutcome::Resolved("1.0.0".to_string())),
                (b, TogetherOutcome::Removed),
            ]
        );
    }

    /// 試す順: 全員を `=` → 頂点だけ `=` → 全員を目標以下の範囲
    #[test]
    fn test_pin_plans_order() {
        let group = vec![
            candidate("web-sys", "0.3.106", "0.3.105"),
            candidate("js-sys", "0.3.106", "0.3.105"),
            candidate("wasm-bindgen", "0.2.129", "0.2.128"),
        ];
        let plans = pin_plans(&group, &[key("web-sys", "0.3.106")]);

        assert_eq!(plans.len(), 3);
        assert_eq!(
            plans[0]
                .pins
                .iter()
                .map(|p| p.requirement.as_str())
                .collect::<Vec<_>>(),
            vec!["=0.3.105", "=0.3.105", "=0.2.128"]
        );
        assert_eq!(plans[0].exact.len(), 3);
        assert_eq!(plans[1].pins.len(), 1);
        assert_eq!(plans[1].pins[0].name, "web-sys");
        assert_eq!(plans[1].exact, vec![key("web-sys", "0.3.106")]);
        assert_eq!(
            plans[2]
                .pins
                .iter()
                .map(|p| p.requirement.as_str())
                .collect::<Vec<_>>(),
            vec![
                ">=0.3.0, <=0.3.105",
                ">=0.3.0, <=0.3.105",
                ">=0.2.0, <=0.2.128"
            ]
        );
        assert!(plans[2].exact.is_empty());
    }

    /// 頂点が全員 (互いに依存しない) なら「頂点だけ」の手は全員の手と同じなので省く
    #[test]
    fn test_pin_plans_skips_tops_plan_when_everyone_is_a_top() {
        let group = vec![
            candidate("a", "1.0.1", "1.0.0"),
            candidate("b", "1.0.1", "1.0.0"),
        ];
        let plans = pin_plans(&group, &[key("a", "1.0.1"), key("b", "1.0.1")]);
        assert_eq!(plans.len(), 2);
        assert!(plans[1].exact.is_empty());
    }

    /// 系列の下限: 1.x は `1.0.0`、0.x は `0.x.0`、0.0.x は範囲にできない
    #[test]
    fn test_series_floor_and_at_most_pin() {
        assert_eq!(series_floor("1.4.9").as_deref(), Some("1.0.0"));
        assert_eq!(series_floor("0.2.128").as_deref(), Some("0.2.0"));
        assert_eq!(series_floor("0.0.7"), None);
        assert_eq!(series_floor("not-a-version"), None);
        assert_eq!(
            at_most_pin(&candidate("tiny", "0.0.8", "0.0.7")).requirement,
            "=0.0.7"
        );
    }

    /// 範囲固定の下端は、install 前の lock の版が系列の最初の版より新しければそちら
    #[test]
    fn test_at_most_pin_uses_version_locked_before_install_as_floor() {
        assert_eq!(
            at_most_pin(&candidate_with_minimum(
                "tokio", "1.53.1", "1.52.0", "1.50.0"
            ))
            .requirement,
            ">=1.50.0, <=1.52.0"
        );
        // 下限と目標が同じなら範囲にならないので `=`
        assert_eq!(
            at_most_pin(&candidate_with_minimum(
                "tokio", "1.53.1", "1.50.0", "1.50.0"
            ))
            .requirement,
            "=1.50.0"
        );
        // 別系列の古い下限は使わない
        assert_eq!(
            at_most_pin(&candidate_with_minimum(
                "syn", "2.0.90", "2.0.80", "1.0.109"
            ))
            .requirement,
            ">=2.0.0, <=2.0.80"
        );
    }

    /// 解き直した結果が install 前の版より古くなったら失敗にする
    #[test]
    fn test_check_resolution_rejects_version_below_minimum() {
        let group = vec![candidate_with_minimum(
            "tokio", "1.53.1", "1.52.0", "1.50.0",
        )];
        let err = check_resolution(&group, &[], &lock(&[("tokio", "1.49.0")])).unwrap_err();
        assert!(err.contains("locked before the install"), "{err}");
        let resolved = check_resolution(&group, &[], &lock(&[("tokio", "1.50.0")])).unwrap();
        assert_eq!(
            resolved[0].1,
            TogetherOutcome::Resolved("1.50.0".to_string())
        );
    }

    /// 残り時間が cargo 1 回分に満たなければ着手しない
    #[test]
    fn test_command_timeout_requires_minimum_slice() {
        let now = Instant::now();
        assert_eq!(command_timeout(now, Duration::from_secs(120)), None);
        let later = now + Duration::from_secs(600);
        assert_eq!(
            command_timeout(later, Duration::from_secs(120)),
            Some(Duration::from_secs(120))
        );
    }
}
