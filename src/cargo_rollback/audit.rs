//! Cargo.lock の公開日監査と差し戻し。時間予算と再試行の状態は lock ごとに保持する。

use super::batch::{RollbackCandidate, TogetherOutcome, resolve_together_tracked};
use super::graph::LockGraph;
use super::scratch::CargoCommand;
use super::series::same_series;
use crate::domain::Language;
use crate::manifest::{RegistryLockEntries, parse_registry_entries};
use crate::registry::RegistryAdapter;
use crate::registry::versions::VersionFetcher;
use crate::update::{VersionInfo, compare_versions};
use futures::{StreamExt, stream};
use indicatif::ProgressBar;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, Instant};

pub(crate) fn read_audit_lock(path: &Path) -> Result<(String, RegistryLockEntries), String> {
    let content = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read Cargo.lock for age audit: {error}"))?;
    let document: toml::Value = toml::from_str(&content)
        .map_err(|error| format!("cannot parse Cargo.lock for age audit: {error}"))?;
    if let Some(packages) = document.get("package") {
        let packages = packages
            .as_array()
            .ok_or("invalid Cargo.lock package list")?;
        if packages.iter().any(|package| {
            package.get("name").and_then(toml::Value::as_str).is_none()
                || package
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .is_none()
                || package
                    .get("source")
                    .is_some_and(|source| source.as_str().is_none())
        }) {
            return Err("invalid Cargo.lock package entry".into());
        }
    } else if document
        .get("version")
        .and_then(toml::Value::as_integer)
        .is_none()
    {
        return Err("invalid Cargo.lock: missing version and package list".into());
    }
    let entries = parse_registry_entries(&content);
    Ok((content, entries))
}

/// `enforce_lock_age_rust` の最大反復回数。
/// 1 回の `cargo update -p --precise` は依存サブツリーを再解決するため、
/// 差し戻しの結果として別の依存が新たに age 違反になるケースがある。
/// 反復することで連鎖を解消するが、無限ループを避けるため上限を設ける。
const MAX_ENFORCE_LOCK_AGE_PASSES: usize = 5;

/// lock ごとの差し戻しと最終全件検証に、それぞれ与える時間の上限。
/// 候補取得 API の待機が全件検証やまとめ解きの枠を食い潰さないよう分ける。
/// 全件検証は sparse index の公開日を並列取得し、未完了なら成功にしない。
pub const LOCK_AGE_AUDIT_BUDGET: Duration = Duration::from_secs(180);

/// age 監査の差し戻しで起動する `cargo update -p --precise` 1 回あたりの上限。
///
/// cargo 自身がレジストリアクセスで固まっても監査フェーズごと止まらないようにする。
/// 通常はインデックス取得込みでも数秒で終わる。
const CARGO_UPDATE_TIMEOUT: Duration = Duration::from_secs(120);

/// 差し戻しに着手するために必要な残り予算。
///
/// `cargo update` に渡すタイムアウトは残り予算でクランプされるため、予算の末尾で
/// 着手すると 1 秒程度で必ずタイムアウトする。条件が変わるまで同じ単独試行は
/// 繰り返さないので、着手できないなら未試行として残す。
const MIN_CARGO_UPDATE_SLICE: Duration = Duration::from_secs(10);

/// まとめ解き ([`resolve_together_tracked`]) に着手するために必要な残り予算。
///
/// まとめ解きは workspace の写しを作り、cargo を少なくとも 3 回 (解き直し・固定の
/// 除去・元の manifest での検証) 続けて起動する。1 件ずつの試行がこの分を食い潰すと、
/// 衝突した一族が 1 件も戻らないまま終わるので、待っている crate があるときは残しておく。
const MIN_TOGETHER_SLICE: Duration = Duration::from_secs(20);

/// judge がこの実行で選んだ版 (crate 名 → 画面に出した更新先)。
///
/// workspace の member ごとに別の版を選ぶことがあるので複数持てる。差し戻し先の
/// 第一候補に使う (`rollback_target`)。
pub type PreferredVersions = HashMap<String, Vec<String>>;

/// `enforce_lock_age_rust` の実行結果
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockAgeAuditResult {
    /// 期間を満たさなかった `(crate 名, 元の版)` ごとの最終結果 (名前 → 版の順)。
    /// 1 件ずつの差し戻しで失敗し、後のまとめ解きで戻せた組は、戻せた結果だけが残る
    pub adjustments: Vec<LockAgeAdjustment>,
    /// 監査を終えた時点の Cargo.lock に残る版のうち公開日を確かめられ
    /// なかった件数 (時間予算 `LOCK_AGE_AUDIT_BUDGET` 切れなど。0 なら全件を確かめた)
    pub unchecked: usize,
    /// 利用者に必ず知らせるべき問題 (まとめ解きの後に元の Cargo.lock を戻せなかった等)
    pub problems: Vec<String>,
    /// 最終 lock に残った版 `(crate 名, 版)` のうち、期間を満たさない・確かめられなかったもの。
    /// 調整の記録 (`from`) ではなく最終状態の証拠なので、差し戻した先の版が残った場合も含む
    pub remaining: BTreeMap<(String, String), RemainingAge>,
}

/// 最終 lock に残った版の、期間の検査結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemainingAge {
    /// 公開日が期間より新しい
    TooNew,
    /// 公開日を確かめられなかった (取得失敗・時間切れ・公開日不明)
    Unverified,
}

/// 最初のチェックポイントと、監査した Cargo 操作が書いた最後のバイト列。
pub(crate) struct AuditReceipt {
    pub(crate) before: Vec<u8>,
    pub(crate) after: Vec<u8>,
}

impl LockAgeAuditResult {
    /// 最終 lock に age 違反や未確認の状態が残ったか。既存版も例外にしない。
    pub fn has_unresolved(&self) -> bool {
        self.unchecked > 0
            || !self.problems.is_empty()
            || self
                .adjustments
                .iter()
                .any(|adjustment| !adjustment.status.is_resolved())
    }
}

/// install 前の Cargo.lock (`baseline`) から見て、新規に入った / バージョンが変わった
/// registry エントリだけを名前順で抽出する。
///
/// 全件監査の順序で、新しく解決された crate を優先するために使う。
fn changed_entries(
    current: &RegistryLockEntries,
    baseline: &RegistryLockEntries,
) -> Vec<(String, Vec<String>)> {
    let mut targets: Vec<(String, Vec<String>)> = current
        .iter()
        .filter_map(|(name, versions)| {
            let known = baseline.get(name);
            let changed: Vec<String> = versions
                .iter()
                .filter(|version| known.is_none_or(|locked| !locked.contains(version)))
                .cloned()
                .collect();
            (!changed.is_empty()).then(|| (name.clone(), changed))
        })
        .collect();
    // HashMap の反復順は非決定的なので、進捗表示と報告順を安定させる
    targets.sort_by(|a, b| a.0.cmp(&b.0));
    targets
}

/// 変更版を先に確認するが、無変更の版も監査対象から外さない。
fn audit_entries(
    current: &RegistryLockEntries,
    baseline: &RegistryLockEntries,
) -> Vec<(String, Vec<String>)> {
    let changed: HashSet<String> = changed_entries(current, baseline)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut targets: Vec<_> = current
        .iter()
        .map(|(name, versions)| (name.clone(), versions.clone()))
        .collect();
    targets.sort_by(|a, b| (!changed.contains(&a.0), &a.0).cmp(&(!changed.contains(&b.0), &b.0)));
    targets
}

/// `enforce_lock_age_rust` が 1 件の依存に対して実施した調整内容
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockAgeAdjustment {
    /// 対象パッケージ名
    pub name: String,
    /// Cargo.lock 上で解決されていたバージョン
    pub from: String,
    /// 最終 lock 上で実際に移動した版。
    pub to: Option<String>,
    /// 差し戻しで要求した版。実際の移動先とは分ける。
    pub target: Option<String>,
    /// 差し戻しの結果
    pub status: LockAgeStatus,
}

/// `enforce_lock_age_rust` の 1 件分ステータス
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockAgeStatus {
    /// 違反バージョンを差し戻した (1 件ずつの `--precise`、またはまとめ解き)
    Downgraded,
    /// まとめて解き直した結果、この crate を必要とする依存が無くなり Cargo.lock から外れた
    Removed,
    /// install 前の lock にあった版へ戻した。ただしその版も期間を満たさない
    /// (install 前から期間内の版が入っていた。install で入った変更だけを取り消した)
    Restored,
    /// install 前の lock から変わっていない版で、期間を満たさない。install 前の版より
    /// 古くはしないので差し戻さない (この実行が入れた版ではない)
    LockedBeforeInstall,
    /// install 時点の組み合わせを保つため、部分的な差し戻しを取り消した。
    Reverted(String),
    /// age 内の代替バージョンが見つからずスキップ (全バージョンが新しい等)
    NoOlderCandidate,
    /// `cargo update` がエラーを返した (resolver 制約違反など。まとめ解きも失敗した場合は
    /// その理由も含む)
    UpdateCommandFailed(String),
    /// Cargo.toml の版要求が、期間を満たす版への差し戻しを許さない (値はその要求)
    BlockedByManifest(String),
    /// 反復の上限や時間予算のために差し戻しを試みなかった (値は理由)
    NotAttempted(String),
    /// レジストリからの release 日取得に失敗
    ReleaseDateUnavailable,
}

impl LockAgeStatus {
    /// 違反が解消した (差し戻した、または lock から外れた) か
    pub fn is_resolved(&self) -> bool {
        matches!(self, LockAgeStatus::Downgraded | LockAgeStatus::Removed)
    }

    /// install 前の版へ戻したが、その版も期間を満たさないか
    pub fn is_restored(&self) -> bool {
        matches!(self, LockAgeStatus::Restored)
    }

    /// 公開日を取得できず、期間を満たすかどうか確かめられなかったか
    pub fn is_unverified(&self) -> bool {
        matches!(self, LockAgeStatus::ReleaseDateUnavailable)
    }
}

/// `audit_lock_age` に渡す、監査 1 回分の入力
#[derive(Clone, Copy)]
pub(crate) struct LockAgeAudit<'a> {
    /// Cargo.lock のあるディレクトリ (workspace root)
    pub(crate) project_dir: &'a Path,
    /// これより後に公開された版を違反とする
    pub(crate) cutoff: chrono::DateTime<chrono::Utc>,
    pub(crate) exemptions: &'a crate::update::AgeExemptions,
    /// install 前の Cargo.lock (差し戻しの下限と確認順序に使う)
    pub(crate) baseline: &'a RegistryLockEntries,
    /// judge がこの実行で選んだ版
    pub(crate) preferred: &'a PreferredVersions,
    /// 公開日の取得元 (本番は crates.io)
    pub(crate) adapter: &'a (dyn RegistryAdapter + Send + Sync),
    /// cargo の起動設定
    pub(crate) cargo: &'a CargoCommand,
    /// 監査全体の時間予算
    pub(crate) budget: Duration,
}

/// 差し戻しの結果を `(crate 名, 元の版)` ごとに 1 件へまとめる記録。
///
/// 同じ組合せが 1 件ずつの試行 → まとめ解きの順に何度か結果を得るので、最後の結果で
/// 上書きする。報告順を安定させるため名前 → 版の順に並べる
#[derive(Default)]
struct AdjustmentLog {
    entries: BTreeMap<(String, String), LockAgeAdjustment>,
}

impl AdjustmentLog {
    /// 結果を記録する (既にあれば上書き)
    fn record(&mut self, name: &str, from: &str, to: Option<String>, status: LockAgeStatus) {
        let (to, target) = if status.is_resolved() || status.is_restored() {
            (to.clone(), to)
        } else {
            (None, to)
        };
        self.entries.insert(
            (name.to_string(), from.to_string()),
            LockAgeAdjustment {
                name: name.to_string(),
                from: from.to_string(),
                to,
                target,
                status,
            },
        );
    }

    /// まだ結果が無いときだけ記録する
    fn record_if_absent(
        &mut self,
        name: &str,
        from: &str,
        to: Option<String>,
        status: LockAgeStatus,
    ) {
        if !self.contains(name, from) {
            self.record(name, from, to, status);
        }
    }

    /// その組の結果が既にあるか
    fn contains(&self, name: &str, from: &str) -> bool {
        self.entries
            .contains_key(&(name.to_string(), from.to_string()))
    }

    /// 差し戻したと記録した組を最終の lock (`entries`) と突き合わせる。元の版がまだ lock に
    /// 残っていれば差し戻せていないので失敗に直し、動いていれば行き先を最終の版に合わせる
    /// (後の差し戻しでさらに動いた場合も、報告を lock の実態に揃える)
    fn reconcile_with_lock(&mut self, entries: &RegistryLockEntries) {
        for adjustment in self.entries.values_mut() {
            if !adjustment.status.is_resolved()
                && entries
                    .get(&adjustment.name)
                    .is_some_and(|versions| versions.contains(&adjustment.from))
            {
                continue;
            }
            let (to, status) =
                match locked_after_rollback(entries, &adjustment.name, &adjustment.from) {
                    None => (
                        None,
                        LockAgeStatus::UpdateCommandFailed(format!(
                            "Cargo.lock still has {} {} after the rollback",
                            adjustment.name, adjustment.from
                        )),
                    ),
                    Some(None) => (None, LockAgeStatus::Removed),
                    Some(Some(version))
                        if compare_versions(&version, &adjustment.from)
                            == std::cmp::Ordering::Less =>
                    {
                        (Some(version), LockAgeStatus::Downgraded)
                    }
                    Some(Some(version)) => (
                        None,
                        LockAgeStatus::UpdateCommandFailed(format!(
                            "{} is now locked at {version}, which is not older than {}",
                            adjustment.name, adjustment.from
                        )),
                    ),
                };
            adjustment.to = to;
            adjustment.status = status;
        }
    }

    fn into_adjustments(self) -> Vec<LockAgeAdjustment> {
        self.entries.into_values().collect()
    }
}

/// 差し戻し先を決める。
///
/// judge がこの実行で選んだ版 (`preferred`) のうち、期間を満たし、今の版より古く、
/// 同じ semver 系列にあるものがあれば、その最大を採る。表示した更新先と lock の版が
/// 一致し、judge が OSV や `--max-change` で退けた版を差し戻しで選び直すこともない。
/// 該当が無ければ、同じ semver 系列で期間を満たす版のうち今の版より古い最新
/// ([`pick_older_within_age`])。
///
/// どちらの場合も、install 前の lock (`before`) にあった利用可能な同系列の版 ([`version_floor`])
/// より古くはしない。manifest の版要求が lock より古いまま (`^1.40.0` で lock は 1.50.0)
/// judge が `--max-change` で 1.40.x を選んだ場合や、install 前から期間内の版が入っていた
/// 場合に、install 前より古い版まで下げると、それまで使えていた API が消えてビルドが壊れる。
/// 下限より古い版しか無ければ install 前の版へ戻す (depup が入れた変更だけを取り消す)
pub(crate) fn rollback_target(
    available: &[VersionInfo],
    current: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
    preferred: Option<&[String]>,
    before: Option<&[String]>,
    exemptions: &crate::update::AgeExemptions,
) -> Option<String> {
    let floor = version_floor(available, before, current);
    let at_least_floor = |version: &str| {
        floor
            .as_deref()
            .is_none_or(|floor| compare_versions(version, floor) != std::cmp::Ordering::Less)
    };
    let from_judge = preferred
        .unwrap_or_default()
        .iter()
        .filter(|version| same_series(version, current))
        .filter(|version| compare_versions(version, current) == std::cmp::Ordering::Less)
        .filter(|version| at_least_floor(version))
        .filter(|version| {
            available.iter().any(|info| {
                compare_versions(&info.version, version) == std::cmp::Ordering::Equal
                    && exemptions.admits(Language::Rust, "", info, cutoff)
            })
        })
        .max_by(|a, b| compare_versions(a, b));
    if let Some(version) = from_judge {
        return Some(version.clone());
    }
    match pick_older_within_age(available, current, cutoff, exemptions) {
        Some(version) if same_series(&version, current) && at_least_floor(&version) => {
            Some(version)
        }
        // 期間を満たす版が下限より古い (または無い) なら、install 前の版に戻す
        _ => floor,
    }
}

/// install 前の lock (`before`) にあった、`current` と同じ semver 系列で `current` 以下の版の
/// うち利用可能な最新。yank 済み・公開日不明などで取得一覧にない版は、下限にも戻し先にも使わない。
fn version_floor(
    available: &[VersionInfo],
    before: Option<&[String]>,
    current: &str,
) -> Option<String> {
    before
        .unwrap_or_default()
        .iter()
        .filter(|version| same_series(version, current))
        .filter(|version| compare_versions(version, current) != std::cmp::Ordering::Greater)
        .filter(|version| {
            available
                .iter()
                .any(|info| compare_versions(&info.version, version) == std::cmp::Ordering::Equal)
        })
        .max_by(|a, b| compare_versions(a, b))
        .cloned()
}

/// 差し戻しの後の lock (`entries`) で、`name` の今の版 (`current`) が外れたかを調べる。
/// 外れていなければ None。外れていれば、同じ semver 系列で lock に残った版
/// (その系列の版が無ければ `Some(None)` = lock から外れた)
fn locked_after_rollback(
    entries: &RegistryLockEntries,
    name: &str,
    current: &str,
) -> Option<Option<String>> {
    let versions = entries.get(name).map(Vec::as_slice).unwrap_or_default();
    if versions.iter().any(|version| version == current) {
        return None;
    }
    Some(
        versions
            .iter()
            .find(|version| same_series(version, current))
            .cloned(),
    )
}

/// まとめ解きを同じ条件で繰り返さないための指紋 (lock の内容と、解こうとした候補の集合)。
///
/// lock だけを見ると、lock が同じまま新しい候補が加わった組を一度も試さず、逆に無関係な
/// 差し戻しで lock が変わるたびに同じ組を丸ごと試し直してしまう
fn together_fingerprint(lock: &str, candidates: &[RollbackCandidate]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut keys: Vec<(&str, &str, &str)> = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.name.as_str(),
                candidate.current.as_str(),
                candidate.target.as_str(),
            )
        })
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    lock.hash(&mut hasher);
    keys.hash(&mut hasher);
    hasher.finish()
}

/// 現在の lock バージョンより古く、かつ cutoff 以前にリリースされた
/// 候補の中から semver 最新のものを選ぶ。
/// プレリリースは除外する。
pub(crate) fn pick_older_within_age(
    available: &[VersionInfo],
    current: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
    exemptions: &crate::update::AgeExemptions,
) -> Option<String> {
    let mut best: Option<&VersionInfo> = None;
    for v in available {
        if v.is_prerelease() {
            continue;
        }
        if !exemptions.admits(Language::Rust, "", v, cutoff) {
            continue;
        }
        // 現在の lock バージョンと同じもしくはそれより新しいものは対象外
        if compare_versions(&v.version, current) != std::cmp::Ordering::Less {
            continue;
        }
        best = match best {
            None => Some(v),
            Some(b) => {
                if compare_versions(&v.version, &b.version) == std::cmp::Ordering::Greater {
                    Some(v)
                } else {
                    Some(b)
                }
            }
        };
    }
    best.map(|v| v.version.clone())
}

/// `cargo update -p <name>@<current> --precise <version>` を実行する。
/// resolver 制約違反など失敗ケースでは stderr を保持した `UpdateCommandFailed` を返す。
///
/// 同名クレートが複数バージョン lock されている場合 (`syn 1.x` + `syn 2.x` 等) に
/// `-p <name>` だけだと cargo が "ambiguous package spec" で失敗するため、
/// 現在バージョン付きの完全修飾 spec で対象を一意にする。
///
/// `CargoCommand::run` は `tokio::process::Command` で起動するので、`cargo update` の
/// 長時間実行で tokio エグゼキュータのワーカースレッドがブロックされて他の async タスク
/// (HTTP リクエスト等) が止まることはない。タイムアウトで待つのをやめたときは、
/// レジストリ待ちの cargo を残さない (kill_on_drop)。
async fn run_cargo_update_precise(
    cargo: &CargoCommand,
    project_dir: &Path,
    name: &str,
    current: &str,
    version: &str,
    timeout: Duration,
) -> LockAgeStatus {
    let spec = format!("{name}@{current}");
    let args = [
        OsStr::new("update"),
        OsStr::new("-p"),
        OsStr::new(&spec),
        OsStr::new("--precise"),
        OsStr::new(version),
    ];
    match cargo.run(project_dir, &args, timeout).await {
        Ok(_) => LockAgeStatus::Downgraded,
        Err(super::scratch::CargoError::TimedOut(timeout)) => {
            LockAgeStatus::NotAttempted(format!("cargo update timed out after {timeout:?}"))
        }
        Err(error) => LockAgeStatus::UpdateCommandFailed(error.to_string()),
    }
}

/// lock ごとの監査状態。取得キャッシュだけが実行全体で共有される。
struct AuditState {
    log: AdjustmentLog,
    alone_failures: HashMap<(String, String), AloneFailure>,
    failed_together: HashSet<u64>,
    problems: Vec<String>,
    started: Instant,
    deadline: Instant,
    installed_lock: String,
    admitted: HashSet<(String, String)>,
    changes: restore::AuditChanges,
    #[cfg(test)]
    elapsed_offset: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
}

struct AloneFailure {
    target: String,
    lock: u64,
    message: String,
    conflict: bool,
}

impl AuditState {
    fn new(budget: Duration) -> Self {
        let started = Instant::now();
        Self {
            log: AdjustmentLog::default(),
            alone_failures: HashMap::new(),
            failed_together: HashSet::new(),
            problems: Vec::new(),
            started,
            deadline: started + budget,
            installed_lock: String::new(),
            admitted: HashSet::new(),
            changes: Default::default(),
            #[cfg(test)]
            elapsed_offset: None,
        }
    }

    fn elapsed(&self) -> Duration {
        #[cfg(test)]
        if let Some(offset) = &self.elapsed_offset {
            return self.started.elapsed()
                + Duration::from_millis(offset.load(std::sync::atomic::Ordering::SeqCst));
        }
        self.started.elapsed()
    }
}

#[derive(Default)]
struct AuditPass {
    has_targets: bool,
    together: Vec<RollbackCandidate>,
    any_downgraded: bool,
    budget_exhausted: bool,
    completed: usize,
    alone_successes: usize,
    prioritize_singles: bool,
    paused_for_reserve: bool,
}

/// 監査入力と、通常更新フローと共有するバージョン取得元。
pub(crate) struct LockAgeAuditor<'a> {
    input: &'a LockAgeAudit<'a>,
    versions: &'a VersionFetcher,
    verbose: bool,
    #[cfg(test)]
    elapsed_offset: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
}

impl<'a> LockAgeAuditor<'a> {
    pub(crate) fn new(
        input: &'a LockAgeAudit<'a>,
        versions: &'a VersionFetcher,
        verbose: bool,
    ) -> Self {
        Self {
            input,
            versions,
            verbose,
            #[cfg(test)]
            elapsed_offset: None,
        }
    }

    /// 検査・差し戻しを反復し、最終 lock の状態から結果を確定する。
    #[cfg(test)]
    pub(crate) async fn run(&self, bar: Option<&ProgressBar>) -> LockAgeAuditResult {
        self.run_tracked(bar, &mut None).await
    }

    pub(crate) async fn run_tracked(
        &self,
        bar: Option<&ProgressBar>,
        receipt: &mut Option<AuditReceipt>,
    ) -> LockAgeAuditResult {
        let mut state = AuditState::new(self.input.budget);
        #[cfg(test)]
        {
            state.elapsed_offset = self.elapsed_offset.clone();
        }
        match read_audit_lock(&self.input.project_dir.join("Cargo.lock")) {
            Ok((content, entries)) => {
                state.changes.observe(&content);
                state.installed_lock = content;
                if self.input.budget.is_zero()
                    || changed_entries(&entries, self.input.baseline).is_empty()
                {
                    return self.finalize(state, bar, receipt).await;
                }
            }
            Err(problem) => {
                return LockAgeAuditResult {
                    problems: vec![problem],
                    ..Default::default()
                };
            }
        }
        let snapshot =
            match snapshot::AuditSnapshot::capture(self.input, &state.installed_lock).await {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    state.problems.push(error);
                    return self.finalize(state, bar, receipt).await;
                }
            };
        // 最後の一巡は差し戻さず、直前の解決で入った版を検証する。
        for pass_index in 0..=MAX_ENFORCE_LOCK_AGE_PASSES {
            let verify_only = pass_index == MAX_ENFORCE_LOCK_AGE_PASSES;
            let mut outcome = match self
                .run_pass(&mut state, pass_index, verify_only, bar)
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    state.problems.push(error);
                    break;
                }
            };
            if !outcome.has_targets {
                break;
            }
            if !verify_only
                && let Err(error) = self.retry_pending(&mut state, &mut outcome, bar).await
            {
                state.problems.push(error);
                break;
            }
            if !outcome.together.is_empty()
                && !verify_only
                && let Err(error) = self.resolve_conflicts(&mut state, &mut outcome, bar).await
            {
                state.problems.push(error);
                break;
            }
            if let Some(bar) = bar {
                bar.set_position(outcome.completed as u64);
            }
            if outcome.budget_exhausted || verify_only || !outcome.any_downgraded {
                break;
            }
        }
        self.complete(state, &snapshot, bar, receipt).await
    }

    async fn complete(
        &self,
        mut state: AuditState,
        snapshot: &snapshot::AuditSnapshot,
        bar: Option<&ProgressBar>,
        receipt: &mut Option<AuditReceipt>,
    ) -> LockAgeAuditResult {
        if state.problems.is_empty() {
            self.restore_mixed(&mut state).await;
        }
        if !state.problems.is_empty() {
            snapshot.restore(&mut state);
        }
        self.finalize(state, bar, receipt).await
    }

    /// 一巡分の公開日検査と、単独で差し戻せる候補の適用。
    async fn run_pass(
        &self,
        state: &mut AuditState,
        pass_index: usize,
        verify_only: bool,
        bar: Option<&ProgressBar>,
    ) -> Result<AuditPass, String> {
        let LockAgeAudit {
            project_dir,
            cutoff,
            exemptions,
            baseline,
            preferred,
            adapter,
            budget,
            ..
        } = *self.input;
        let lock_path = project_dir.join("Cargo.lock");
        let (content, entries) = read_audit_lock(&lock_path)?;
        state.changes.observe(&content);
        let keys: Vec<_> = changed_entries(&entries, baseline)
            .into_iter()
            .flat_map(|(name, versions)| {
                versions
                    .into_iter()
                    .map(move |version| (name.clone(), version))
            })
            .collect();
        let targets = LockGraph::from_lock_content(&content).dependents_first(&keys);
        if targets.is_empty() {
            return Ok(AuditPass::default());
        }
        let mut outcome = AuditPass {
            has_targets: true,
            ..Default::default()
        };
        if let Some(b) = bar {
            // 位置を先に戻す。長さを先に縮めると、前パスの位置が残っている間に
            // 描画されて `14/1` のような不整合が一瞬見える
            b.set_position(0);
            b.set_length(targets.len() as u64);
        }
        if pass_index == 0 && self.verbose {
            let message = format!(
                "  {} — {} changed registry crate(s) in Cargo.lock; checking release dates",
                project_dir.display(),
                targets.len()
            );
            match bar {
                Some(b) => b.suspend(|| eprintln!("{message}")),
                None => eprintln!("{message}"),
            }
        }

        'audit: for (index, (name, version)) in targets.iter().enumerate() {
            let versions = std::slice::from_ref(version);
            let elapsed = state.elapsed();
            if elapsed >= budget {
                // 予算切れ: 監査済みの調整は活かしつつ打ち切る。確かめられなかった件数は
                // 最後に最終の lock から数える
                outcome.budget_exhausted = true;
                break;
            }
            // 位置はインデックスから設定する (fetch 失敗時の `continue` を挟んでも
            // 進捗がずれない)
            if let Some(b) = bar {
                b.set_position(index as u64);
                b.set_message(format!("Auditing {}", name));
            }
            // 公開日の取得も残り予算で打ち切る (HTTP クライアント側の上限は予算より長い)
            let fetched =
                tokio::time::timeout(budget - elapsed, self.versions.fetch(adapter, name)).await;
            let Ok(fetched) = fetched else {
                outcome.budget_exhausted = true;
                break;
            };
            let mut all_versions = match fetched {
                Ok(v) => v,
                Err(_) => {
                    for v in versions {
                        state.log.record_if_absent(
                            name,
                            v,
                            None,
                            LockAgeStatus::ReleaseDateUnavailable,
                        );
                    }
                    // fetch 失敗でもこの対象は「処理済み」。ここで進捗を進めないと
                    // 末尾の数件が失敗したときにバーが手前で止まったまま次の
                    // ディレクトリへ移る (全件失敗なら 0 のまま)
                    outcome.completed = index + 1;
                    continue;
                }
            };

            if versions.iter().any(|current| {
                !all_versions.iter().any(|info| {
                    compare_versions(&info.version, current) == std::cmp::Ordering::Equal
                })
            }) {
                // check と install の間に公開された版を、古いキャッシュだけで未確認にしない。
                let remaining = budget.saturating_sub(state.elapsed());
                match tokio::time::timeout(remaining, self.versions.refresh(adapter, name)).await {
                    Ok(Ok(fresh)) => all_versions = fresh,
                    Ok(Err(_)) => {}
                    Err(_) => {
                        outcome.budget_exhausted = true;
                        break;
                    }
                }
            }

            for current in versions {
                let Some(current_info) = all_versions
                    .iter()
                    .find(|v| compare_versions(&v.version, current) == std::cmp::Ordering::Equal)
                else {
                    state.log.record_if_absent(
                        name,
                        current,
                        None,
                        LockAgeStatus::ReleaseDateUnavailable,
                    );
                    continue;
                };

                if exemptions.admits(Language::Rust, name, current_info, cutoff) {
                    state.admitted.insert((name.clone(), current.clone()));
                    continue;
                }

                let Some(target) = rollback_target(
                    &all_versions,
                    current,
                    cutoff,
                    preferred.get(name).map(Vec::as_slice),
                    baseline.get(name).map(Vec::as_slice),
                    exemptions,
                ) else {
                    state
                        .log
                        .record(name, current, None, LockAgeStatus::NoOlderCandidate);
                    continue;
                };
                if compare_versions(&target, current) != std::cmp::Ordering::Less {
                    state.log.record_if_absent(
                        name,
                        current,
                        pick_older_within_age(&all_versions, current, cutoff, exemptions),
                        LockAgeStatus::NotAttempted(format!(
                            "the pre-install locked version {target} is the rollback minimum and does not satisfy --age"
                        )),
                    );
                    continue;
                }
                let candidate = RollbackCandidate {
                    name: name.clone(),
                    current: current.clone(),
                    target: target.clone(),
                    minimum: version_floor(
                        &all_versions,
                        baseline.get(name).map(Vec::as_slice),
                        current,
                    ),
                };

                if verify_only {
                    // 差し戻しの結果がまだ付いていない違反だけを「上限到達」にする
                    // (1 件ずつ・まとめ解きで失敗した理由は上書きしない)
                    state.log.record_if_absent(
                        name,
                        current,
                        Some(target),
                        LockAgeStatus::NotAttempted(format!(
                            "stopped after {MAX_ENFORCE_LOCK_AGE_PASSES} rounds of rollbacks"
                        )),
                    );
                    continue;
                }

                if let Err(error) = self.rollback_one(candidate, state, &mut outcome, bar).await {
                    state.problems.push(error);
                    break 'audit;
                }
                if outcome.budget_exhausted || outcome.paused_for_reserve {
                    break 'audit;
                }
            }
            outcome.completed = index + 1;
        }
        Ok(outcome)
    }

    /// lock または対象が変わったときだけ、単独の差し戻しを再試行する。
    async fn rollback_one(
        &self,
        candidate: RollbackCandidate,
        state: &mut AuditState,
        outcome: &mut AuditPass,
        bar: Option<&ProgressBar>,
    ) -> Result<(), String> {
        let LockAgeAudit {
            project_dir,
            cargo,
            budget,
            ..
        } = *self.input;
        let lock_path = project_dir.join("Cargo.lock");
        let RollbackCandidate {
            name,
            current,
            target,
            ..
        } = &candidate;
        let key = (name.clone(), current.clone());
        let (content, entries) = read_audit_lock(&lock_path)?;
        state.changes.observe(&content);
        if let Some(moved) = locked_after_rollback(&entries, name, current) {
            let status = match &moved {
                None => LockAgeStatus::Removed,
                Some(version) if compare_versions(version, current) == std::cmp::Ordering::Less => {
                    LockAgeStatus::Downgraded
                }
                Some(version) => LockAgeStatus::UpdateCommandFailed(format!(
                    "{name} is now locked at {version}, which is not older than {current}"
                )),
            };
            outcome.any_downgraded |= status.is_resolved();
            outcome.alone_successes += usize::from(status.is_resolved());
            state.log.record(name, current, moved, status);
            state.log.entries.get_mut(&key).unwrap().target = Some(target.clone());
            state.alone_failures.remove(&key);
            return Ok(());
        }
        let mut fingerprint = together_fingerprint(&content, &[]);
        if let Some(failure) = state.alone_failures.get(&key)
            && failure.lock == fingerprint
            && failure.target == *target
        {
            if failure.conflict {
                outcome.together.push(candidate);
            }
            return Ok(());
        }

        // 残り予算が `cargo update` 1 回分に満たないなら着手しない。
        // 同じ lock と目標では繰り返さないため、短く切り詰めたタイムアウトで
        // 試すより、条件が変わった後に十分な時間を与えて再試行する。
        // まとめ解きを待っている crate があるときは、その分の時間も残す
        // (1 件ずつの試行だけで予算を使い切らない)
        let remaining = budget.saturating_sub(state.elapsed());
        let reserve = if outcome.prioritize_singles || outcome.together.is_empty() {
            Duration::ZERO
        } else {
            MIN_TOGETHER_SLICE
        };
        if remaining < MIN_CARGO_UPDATE_SLICE + reserve {
            if reserve != Duration::ZERO && remaining >= MIN_CARGO_UPDATE_SLICE {
                // メタデータ取得の予算をさらに使わず走査を終える。保留中の単独処理には、
                // まとめて解くか決める前に残りの時間を使わせる。
                outcome.together.push(candidate.clone());
                outcome.paused_for_reserve = true;
                return Ok(());
            }
            outcome.budget_exhausted = true;
            return Ok(());
        }

        // `cargo update --precise` は 1 件あたり数秒かかる。バーの位置は
        // 動かないため、何を差し戻し中かはメッセージで示す
        if let Some(b) = bar {
            b.set_message(format!("Rolling back {} {} → {}", name, current, target));
        }
        // 差し戻しの `cargo update` にも残り予算を渡す。ここでクランプしないと
        // 予算切れ直前に始まった 1 件が上限を大きく踏み越える
        let status = run_cargo_update_precise(
            cargo,
            project_dir,
            name,
            current,
            target,
            remaining.min(CARGO_UPDATE_TIMEOUT),
        )
        .await;
        // cargo が成功を返しても lock が変わったとは限らない (directory source
        // への置き換えでは `--precise` が何もせずに exit 0 で終わる)。lock を
        // 読み直し、実際に今の版から外れたことを確かめてから差し戻しとして数える
        let status = match status {
            LockAgeStatus::Downgraded => {
                let (after, locked) = read_audit_lock(&lock_path)?;
                // 対象の版が動かなくても、コマンドが書いた内容はそのコマンドの変更とする。
                state
                    .changes
                    .applied(&content, &after, std::slice::from_ref(&key));
                match locked_after_rollback(&locked, name, current) {
                    Some(moved) => {
                        outcome.any_downgraded = true;
                        outcome.alone_successes += 1;
                        state.alone_failures.remove(&key);
                        let status = if moved.is_some() {
                            LockAgeStatus::Downgraded
                        } else {
                            LockAgeStatus::Removed
                        };
                        state.log.record(name, current, moved, status);
                        state.log.entries.get_mut(&key).unwrap().target = Some(target.clone());
                        return Ok(());
                    }
                    None => {
                        // このコマンド自身の書き込み以外の変更があったときだけ再試行する。
                        fingerprint = together_fingerprint(&after, &[]);
                        LockAgeStatus::UpdateCommandFailed(format!(
                            "cargo update --precise {target} exited successfully, but Cargo.lock still has {name} {current}"
                        ))
                    }
                }
            }
            other => other,
        };
        match status {
            LockAgeStatus::UpdateCommandFailed(message) => {
                state.alone_failures.insert(
                    key,
                    AloneFailure {
                        target: target.clone(),
                        lock: fingerprint,
                        message: message.clone(),
                        conflict: true,
                    },
                );
                outcome.together.push(candidate.clone());
                state.log.record(
                    name,
                    current,
                    Some(target.clone()),
                    LockAgeStatus::UpdateCommandFailed(message),
                );
            }
            LockAgeStatus::NotAttempted(message) => {
                // タイムアウトだけでは解決の衝突と断定できないが、後の走査で同じ lock と
                // 対象を再試行してはならない。
                state.alone_failures.insert(
                    key,
                    AloneFailure {
                        target: target.clone(),
                        lock: fingerprint,
                        message: message.clone(),
                        conflict: false,
                    },
                );
                state.log.record(
                    name,
                    current,
                    Some(target.clone()),
                    LockAgeStatus::NotAttempted(message),
                );
            }
            other => state.log.record(name, current, Some(target.clone()), other),
        }
        Ok(())
    }

    async fn retry_pending(
        &self,
        state: &mut AuditState,
        outcome: &mut AuditPass,
        bar: Option<&ProgressBar>,
    ) -> Result<(), String> {
        outcome.paused_for_reserve = false;
        while !outcome.budget_exhausted && !outcome.together.is_empty() {
            let (content, _) = read_audit_lock(&self.input.project_dir.join("Cargo.lock"))?;
            let mut pending: BTreeMap<_, _> = std::mem::take(&mut outcome.together)
                .into_iter()
                .map(|candidate| {
                    (
                        (candidate.name.clone(), candidate.current.clone()),
                        candidate,
                    )
                })
                .collect();
            // lock が変わった後の再試行は、まとめ解き用の予約時間より常に優先する。
            // 以前に失敗した候補が再び失敗した後の再試行も含む。
            // 残り20秒未満でも、単純な依存の連鎖なら処理を完了できる。
            let order = LockGraph::from_lock_content(&content)
                .dependents_first(&pending.keys().cloned().collect::<Vec<_>>());
            let successes = outcome.alone_successes;
            for key in order {
                let candidate = pending.remove(&key).unwrap();
                outcome.prioritize_singles = true;
                let attempt = self.rollback_one(candidate, state, outcome, bar).await;
                outcome.prioritize_singles = false;
                attempt?;
                if outcome.budget_exhausted {
                    outcome.together.extend(pending.into_values());
                    break;
                }
            }
            if outcome.alone_successes == successes {
                break;
            }
        }
        Ok(())
    }

    /// 同じ lock と候補の集合で再試行せず、衝突した依存をまとめて解く。
    async fn resolve_conflicts(
        &self,
        state: &mut AuditState,
        outcome: &mut AuditPass,
        bar: Option<&ProgressBar>,
    ) -> Result<(), String> {
        let LockAgeAudit {
            project_dir,
            cargo,
            budget,
            ..
        } = *self.input;
        let lock_path = project_dir.join("Cargo.lock");
        // 同じパスの 1 件ずつの差し戻しで lock は変わっているので、直前の状態で判定する
        let (content, _) = read_audit_lock(&lock_path)?;
        state.changes.observe(&content);
        let fingerprint = together_fingerprint(&content, &outcome.together);
        let remaining = budget.saturating_sub(state.elapsed());
        if remaining < MIN_TOGETHER_SLICE {
            for candidate in &outcome.together {
                state.log.record_if_absent(
                    &candidate.name,
                    &candidate.current,
                    Some(candidate.target.clone()),
                    LockAgeStatus::NotAttempted(
                        "audit time budget ran out before resolving together".to_string(),
                    ),
                );
            }
            outcome.budget_exhausted = true;
        } else if !state.failed_together.contains(&fingerprint) {
            if let Some(b) = bar {
                b.set_message(format!(
                    "Resolving {} crate(s) together",
                    outcome.together.len()
                ));
            }
            let (report, changes) = resolve_together_tracked(
                project_dir,
                &outcome.together,
                cargo,
                state.deadline,
                CARGO_UPDATE_TIMEOUT,
            )
            .await;
            for change in changes {
                state
                    .changes
                    .committed(&change.before, &change.after, &change.selected);
            }
            let (after, _) = read_audit_lock(&lock_path)?;
            state.changes.observe(&after);
            state.problems.extend(report.problems);
            let mut resolved_any = false;
            for (candidate, outcome) in report.outcomes {
                let key = (candidate.name.clone(), candidate.current.clone());
                match outcome {
                    TogetherOutcome::Resolved(version) => {
                        resolved_any = true;
                        state.alone_failures.remove(&key);
                        state.log.record(
                            &candidate.name,
                            &candidate.current,
                            Some(version),
                            LockAgeStatus::Downgraded,
                        );
                        state.log.entries.get_mut(&key).unwrap().target =
                            Some(candidate.target.clone());
                    }
                    TogetherOutcome::Removed => {
                        resolved_any = true;
                        state.alone_failures.remove(&key);
                        state.log.record(
                            &candidate.name,
                            &candidate.current,
                            None,
                            LockAgeStatus::Removed,
                        );
                    }
                    TogetherOutcome::BlockedByManifest(requirement) => {
                        state.alone_failures.remove(&key);
                        state.log.record(
                            &candidate.name,
                            &candidate.current,
                            Some(candidate.target.clone()),
                            LockAgeStatus::BlockedByManifest(requirement),
                        );
                    }
                    TogetherOutcome::Failed(reason) => {
                        let message = match state
                            .alone_failures
                            .get(&key)
                            .map(|failure| &failure.message)
                        {
                            Some(alone) => {
                                format!("{alone}\n(resolving together also failed: {reason})")
                            }
                            None => format!("resolving together failed: {reason}"),
                        };
                        state.log.record(
                            &candidate.name,
                            &candidate.current,
                            Some(candidate.target.clone()),
                            LockAgeStatus::UpdateCommandFailed(message),
                        );
                    }
                    // 時間切れは cargo の失敗ではない。1 件ずつで衝突した理由があれば
                    // それを残し、無ければ試さなかったことを記録する
                    TogetherOutcome::NotAttempted(reason) => {
                        let status = match state
                            .alone_failures
                            .get(&key)
                            .map(|failure| &failure.message)
                        {
                            Some(alone) => LockAgeStatus::UpdateCommandFailed(format!(
                                "{alone}\n(resolving together was not finished: {reason})"
                            )),
                            None => LockAgeStatus::NotAttempted(reason),
                        };
                        state.log.record(
                            &candidate.name,
                            &candidate.current,
                            Some(candidate.target.clone()),
                            status,
                        );
                    }
                }
            }
            if resolved_any {
                outcome.any_downgraded = true;
            } else {
                state.failed_together.insert(fingerprint);
            }
        } else {
            // 同じ lock の状態で既に失敗している。1 件ずつも試していない
            // (予算の都合でまとめ解きに回した) crate が報告から漏れないようにする
            for candidate in &outcome.together {
                state.log.record_if_absent(
                    &candidate.name,
                    &candidate.current,
                    Some(candidate.target.clone()),
                    LockAgeStatus::NotAttempted(
                        "resolving together already failed for the same Cargo.lock".to_string(),
                    ),
                );
            }
        }
        Ok(())
    }

    /// 差し戻しとは別の時間予算で、最終 lock の全件を検証する。
    async fn finalize(
        &self,
        state: AuditState,
        bar: Option<&ProgressBar>,
        receipt: &mut Option<AuditReceipt>,
    ) -> LockAgeAuditResult {
        let LockAgeAudit {
            project_dir,
            cutoff,
            exemptions,
            baseline,
            adapter,
            budget,
            ..
        } = *self.input;
        let lock_path = project_dir.join("Cargo.lock");
        let AuditState {
            mut log,
            mut problems,
            changes,
            installed_lock,
            ..
        } = state;
        let (content, final_entries) = match read_audit_lock(&lock_path) {
            Ok(entries) => entries,
            Err(error) => {
                if !problems.contains(&error) {
                    problems.push(error);
                }
                return LockAgeAuditResult {
                    problems,
                    ..Default::default()
                };
            }
        };
        log.reconcile_with_lock(&final_entries);
        let targets = audit_entries(&final_entries, baseline);
        let deadline = tokio::time::Instant::now() + budget;
        if let Some(bar) = bar {
            bar.set_position(0);
            bar.set_length(targets.len() as u64);
            bar.set_message("Verifying all registry crates in final Cargo.lock");
        }
        let mut metadata = HashMap::new();
        // 実行内の候補キャッシュは使えるが、候補から落ちた yank 済みの版は別途取得する。
        {
            let requests = stream::iter(targets.iter().map(|(name, locked)| async move {
                let cached = self.versions.cached(adapter, name).await;
                if let Some(cached) = cached
                    && locked
                        .iter()
                        .all(|locked| cached.iter().any(|info| info.version == *locked))
                {
                    return (name.clone(), Some(cached));
                }
                let fetched = adapter
                    .fetch_locked_versions(name, locked, (!exemptions.is_empty()).then_some(cutoff))
                    .await
                    .ok();
                (name.clone(), fetched)
            }))
            .buffer_unordered(8);
            tokio::pin!(requests);
            while tokio::time::Instant::now() < deadline {
                let Ok(Some((name, known))) =
                    tokio::time::timeout_at(deadline, requests.next()).await
                else {
                    break;
                };
                metadata.insert(name.clone(), known);
                if let Some(bar) = bar {
                    bar.set_position(metadata.len() as u64);
                    bar.set_message(format!("Verifying {name}"));
                }
            }
        }
        // 実際の移動先を最終状態で判定する。既存の若い版へ戻しても成功にはしない。
        for adjustment in log.entries.values_mut() {
            if adjustment.status != LockAgeStatus::Downgraded {
                continue;
            }
            let Some(to) = &adjustment.to else {
                continue;
            };
            let info = metadata
                .get(&adjustment.name)
                .and_then(Option::as_ref)
                .and_then(|known| known.iter().find(|info| info.version == *to));
            adjustment.status = match info {
                Some(info) if exemptions.admits(Language::Rust, &adjustment.name, info, cutoff) => {
                    LockAgeStatus::Downgraded
                }
                Some(_)
                    if baseline
                        .get(&adjustment.name)
                        .is_some_and(|versions| versions.contains(to)) =>
                {
                    LockAgeStatus::Restored
                }
                Some(_) => LockAgeStatus::NotAttempted(format!(
                    "final locked version {to} does not satisfy --age"
                )),
                None => LockAgeStatus::ReleaseDateUnavailable,
            };
        }
        let mut unchecked = 0;
        let mut remaining = BTreeMap::new();
        for (name, versions) in targets {
            let known = metadata.get(&name).and_then(Option::as_ref);
            for version in versions {
                let info =
                    known.and_then(|known| known.iter().find(|info| info.version == version));
                if info.is_some_and(|info| exemptions.admits(Language::Rust, &name, info, cutoff)) {
                    // 最終状態を確認できれば途中の取得失敗は残さない。
                    log.entries.remove(&(name.clone(), version.clone()));
                    continue;
                }
                remaining.insert(
                    (name.clone(), version.clone()),
                    if info.is_some() {
                        RemainingAge::TooNew
                    } else {
                        RemainingAge::Unverified
                    },
                );
                // 差し戻し先の若い版を別件で重複報告しない。
                if log.entries.values().any(|adjustment| {
                    adjustment.name == name && adjustment.to.as_ref() == Some(&version)
                }) {
                    continue;
                }
                if !metadata.contains_key(&name) {
                    unchecked += 1;
                }
                let status = match info {
                    None => LockAgeStatus::ReleaseDateUnavailable,
                    Some(_)
                        if baseline
                            .get(&name)
                            .is_some_and(|versions| versions.contains(&version)) =>
                    {
                        LockAgeStatus::LockedBeforeInstall
                    }
                    Some(_) => {
                        LockAgeStatus::NotAttempted("the audit ended before rolling it back".into())
                    }
                };
                log.record_if_absent(&name, &version, None, status);
            }
        }
        if std::fs::read_to_string(&lock_path)
            .map_or(true, |final_content| final_content != content)
        {
            problems.push("Cargo.lock changed during final age verification; rerun --install to verify the new state".into());
        } else if changes.allows_snapshot_restore(Some(content.as_bytes())) {
            *receipt = Some(AuditReceipt {
                before: installed_lock.into_bytes(),
                after: content.into_bytes(),
            });
        }
        LockAgeAuditResult {
            adjustments: log.into_adjustments(),
            unchecked,
            problems,
            remaining,
        }
    }
}

#[cfg(test)]
mod helper_tests;

mod restore;
mod snapshot;
