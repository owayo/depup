//! 更新オーケストレータ - 更新ワークフロー全体の調整
//!
//! このモジュールは以下を提供する:
//! - ワークフロー調整: 検出 → パース → フェッチ → 判定 → 書き込み
//! - レート制限付き並列レジストリクエリ
//! - ドライランモード対応
//! - 言語・パッケージフィルタの適用
//! - 部分的な継続を伴うエラーハンドリング

use crate::cargo_rollback::batch::{RollbackCandidate, TogetherOutcome, resolve_together};
use crate::cargo_rollback::scratch::CargoCommand;
use crate::cargo_rollback::series::same_series;
use crate::cli::CliArgs;
use crate::domain::{
    Dependency, GitReference, Language, ManifestUpdateResult, SkipReason, UpdateResult,
    UpdateSummary,
};
use crate::global_config::GlobalConfig;

mod age;
use crate::manifest::{
    ManifestInfo, ManifestWriter, MiseSettings, RegistryLockEntries, WriteResult, detect_manifests,
    find_cargo_lock_upward, get_parser, has_mise_config, parse_registry_entries, read_git_entries,
};
use crate::osv::{OsvCheck, OsvChecker};
use crate::progress::Progress;
use crate::registry::{
    CratesIoAdapter, CratesIoRateLimit, GitHubTagsAdapter, GitRemote, GoProxyAdapter, HttpClient,
    MavenCentralAdapter, MiseAdapter, NpmAdapter, PackagistAdapter, PyPIAdapter, RegistryAdapter,
    RubyGemsAdapter,
};
use crate::tauri_sync::{TAURI_CRATE, TAURI_NPM_PACKAGES, TauriVersionSync};
use crate::update::{
    AgePolicy, UpdateFilter, UpdateJudge, VersionInfo, compare_dependency_versions,
    compare_versions,
};
use futures::stream::{self, StreamExt};
use indicatif::ProgressBar;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};

/// レジストリリクエストのデフォルト同時実行数
const DEFAULT_CONCURRENCY: usize = 10;

/// crates.io 用の同時実行数 (レート制限あり)
const CRATES_IO_CONCURRENCY: usize = 1;

/// バージョンチェックの並列度上限 (マニフェスト内の依存関係に対して)。
/// 依存数が少ない場合はそれに合わせて並列度を下げる (`dep_count.clamp(1, 4)`)。
const MAX_VERSION_CHECK_CONCURRENCY: usize = 4;

/// マニフェスト内の依存数から並列度を計算する。
/// 最小 1、最大 `MAX_VERSION_CHECK_CONCURRENCY`。
fn version_check_concurrency(dep_count: usize) -> usize {
    dep_count.clamp(1, MAX_VERSION_CHECK_CONCURRENCY)
}

/// `enforce_lock_age_rust` の最大反復回数。
/// 1 回の `cargo update -p --precise` は依存サブツリーを再解決するため、
/// 差し戻しの結果として別の依存が新たに age 違反になるケースがある。
/// 反復することで連鎖を解消するが、無限ループを避けるため上限を設ける。
const MAX_ENFORCE_LOCK_AGE_PASSES: usize = 5;

/// `enforce_lock_age_rust` がレジストリ照会に使える時間の上限。
///
/// crates.io は crawler policy により 1 リクエスト/秒へ直列化されるため、監査対象が
/// 数百件に膨らむと数分間ネットワーク待ちで無音になり、利用者からはハングと区別が
/// つかない。通常は差分監査 (`changed_entries`) で数件に収まるが、Cargo.lock を
/// 新規生成した直後など対象が大量になるケースの安全弁として上限を設ける。
/// 予算切れの場合は監査済みの分を適用したうえで、未検証件数を呼び出し側へ返す。
pub const LOCK_AGE_AUDIT_BUDGET: Duration = Duration::from_secs(180);

/// age 監査の差し戻しで起動する `cargo update -p --precise` 1 回あたりの上限。
///
/// cargo 自身がレジストリアクセスで固まっても監査フェーズごと止まらないようにする。
/// 通常はインデックス取得込みでも数秒で終わる。
const CARGO_UPDATE_TIMEOUT: Duration = Duration::from_secs(120);

/// 差し戻しに着手するために必要な残り予算。
///
/// `cargo update` に渡すタイムアウトは残り予算でクランプされるため、予算の末尾で
/// 着手すると 1 秒程度で必ずタイムアウトする。1 件ずつ試した組合せは二度と
/// 1 件ずつでは再試行されないため、本来差し戻せた依存が「失敗」として恒久的に
/// 確定してしまう。着手できないなら未検証として残す方が良い。
const MIN_CARGO_UPDATE_SLICE: Duration = Duration::from_secs(10);

/// まとめ解き ([`resolve_together`]) に着手するために必要な残り予算。
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

/// バージョン情報のキャッシュ (言語, パッケージ名) をキーとする
pub type VersionCache = Arc<Mutex<HashMap<(Language, String), Vec<VersionInfo>>>>;
type VersionFetchLocks = Arc<Mutex<HashMap<(Language, String), Arc<Mutex<()>>>>>;

/// パース済みマニフェスト (parse_phase → check_phase の受け渡し用)
struct ParsedManifest<'a> {
    info: &'a ManifestInfo,
    dependencies: Vec<Dependency>,
}

/// 更新ワークフローを調整するオーケストレータ
pub struct Orchestrator {
    /// 設定用CLI引数
    args: CliArgs,
    evaluated_at: chrono::DateTime<chrono::Utc>,
    /// レジストリリクエスト用HTTPクライアント
    client: HttpClient,
    /// 汎用同時実行制御用セマフォ
    general_semaphore: Arc<Semaphore>,
    /// crates.io 専用レート制限セマフォ
    crates_io_semaphore: Arc<Semaphore>,
    /// crates.io の 1 リクエスト/秒 間隔を実行全体で共有する状態。
    /// アダプタごとに持たせるとマニフェスト境界やフェーズ境界で間隔がリセットされる
    crates_io_rate_limit: Arc<CratesIoRateLimit>,
    /// ディレクトリ間で共有されるバージョンキャッシュ
    version_cache: VersionCache,
    /// 同一パッケージの取得を一つにまとめるキー単位のロック
    version_fetch_locks: VersionFetchLocks,
    /// URL 単位でキャッシュされる git ls-remote クライアント
    git_remote: GitRemote,
    /// OSV チェッカー (`args.osv` が true のときのみ初期化)
    osv_checker: Option<OsvChecker>,
    /// グローバル設定 (~/.config/depup/config.toml)
    global_config: Option<GlobalConfig>,
}

/// オーケストレータの実行結果
pub struct OrchestratorResult {
    /// 全結果を含む更新サマリ
    pub summary: UpdateSummary,
    /// 各マニフェストの書き込み結果
    pub write_results: Vec<WriteResult>,
    /// 処理中に発生したエラー
    pub errors: Vec<OrchestratorError>,
}

/// オーケストレーション中に発生しうるエラー
#[derive(Debug)]
pub enum OrchestratorError {
    /// HTTPクライアントの作成に失敗
    HttpClientError(String),
    /// マニフェストの検出に失敗
    ManifestDetectionError(String),
    /// マニフェストのパースに失敗
    ManifestParseError { path: String, message: String },
    /// レジストリからのバージョン取得に失敗
    RegistryError { package: String, message: String },
    /// マニフェストの書き込みに失敗
    WriteError { path: String, message: String },
    /// OSV 脆弱性チェックに失敗または脆弱性を検出
    OsvWarning { package: String, message: String },
}

impl std::fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrchestratorError::HttpClientError(msg) => write!(f, "HTTP client error: {}", msg),
            OrchestratorError::ManifestDetectionError(msg) => {
                write!(f, "Manifest detection error: {}", msg)
            }
            OrchestratorError::ManifestParseError { path, message } => {
                write!(f, "Failed to parse {}: {}", path, message)
            }
            OrchestratorError::RegistryError { package, message } => {
                write!(f, "Failed to fetch {}: {}", package, message)
            }
            OrchestratorError::WriteError { path, message } => {
                write!(f, "Failed to write {}: {}", path, message)
            }
            OrchestratorError::OsvWarning { package, message } => {
                write!(f, "OSV check for {}: {}", package, message)
            }
        }
    }
}

impl std::error::Error for OrchestratorError {}

impl Orchestrator {
    /// 指定されたCLI引数で新しいオーケストレータを作成する
    pub fn new(args: CliArgs) -> Result<Self, OrchestratorError> {
        let client =
            HttpClient::new().map_err(|e| OrchestratorError::HttpClientError(e.to_string()))?;

        let osv_checker = if args.osv {
            Some(OsvChecker::new().map_err(OrchestratorError::HttpClientError)?)
        } else {
            None
        };

        Ok(Self {
            args,
            evaluated_at: chrono::Utc::now(),
            client,
            general_semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
            crates_io_semaphore: Arc::new(Semaphore::new(CRATES_IO_CONCURRENCY)),
            crates_io_rate_limit: Arc::new(CratesIoRateLimit::new()),
            version_cache: Arc::new(Mutex::new(HashMap::new())),
            version_fetch_locks: Arc::new(Mutex::new(HashMap::new())),
            git_remote: GitRemote::new(),
            osv_checker,
            global_config: None,
        })
    }

    /// グローバル設定をセットする (CLI > プロジェクト設定 の解決に使う)
    pub fn with_global_config(mut self, config: Option<GlobalConfig>) -> Self {
        self.global_config = config;
        self
    }

    /// Rust プロジェクトの Cargo.lock を走査し、`--age` を満たさない crate を
    /// 期間を満たす版へ差し戻す。
    ///
    /// 典型的な用途: `depup --age 2w --install` 実行時、`cargo update` が
    /// semver 解決で 2 週間以内にリリースされた版 (直接依存・推移依存とも) を
    /// 引き込んでしまう場合に、それらを age 境界以前の版へ戻す。
    ///
    /// まず 1 件ずつ `cargo update -p --precise <older_version>` で戻し、互いを `=` で
    /// 固定し合う一族 (wasm-bindgen など) のように 1 件ずつでは衝突して戻せないものは、
    /// パスの最後にまとめて解き直す ([`resolve_together`])。
    ///
    /// 1 つの依存を差し戻すと cargo が依存サブツリーを再解決し、別の依存が
    /// 新たに age 違反になる可能性があるため、最大 `MAX_ENFORCE_LOCK_AGE_PASSES` 回
    /// 反復する。変化がなくなった時点で終了する。
    ///
    /// `preferred` は judge がこの実行で選んだ版 (画面に出した更新先)。差し戻し先の
    /// 第一候補にするので、差し戻せれば表示と lock の版が一致し、OSV や
    /// `--max-change` の判定も守られる。
    ///
    /// 戻り値: 期間を満たさなかった組ごとの最終結果、監査を終えた時点の Cargo.lock で
    /// 公開日を確かめられなかった版の数、利用者に必ず知らせるべき問題 (まとめ解きの後に
    /// 元の Cargo.lock を戻せなかった等)。結果は最終の Cargo.lock の状態から組み立て直す
    pub async fn enforce_lock_age_rust(
        &self,
        project_dir: &Path,
        min_age: Duration,
        baseline: &RegistryLockEntries,
        preferred: &PreferredVersions,
        bar: Option<&ProgressBar>,
    ) -> LockAgeAuditResult {
        let mut policy = self.resolved_age_policy_for(project_dir);
        policy.min_age = Some(min_age);
        self.enforce_lock_age_rust_with_policy(project_dir, &policy, baseline, preferred, bar)
            .await
    }

    /// 共有 lock の全メンバーから解決済みの制約を使って監査する。
    pub async fn enforce_lock_age_rust_with_policy(
        &self,
        project_dir: &Path,
        policy: &AgePolicy,
        baseline: &RegistryLockEntries,
        preferred: &PreferredVersions,
        bar: Option<&ProgressBar>,
    ) -> LockAgeAuditResult {
        let Some(cutoff) = policy.cutoff() else {
            return LockAgeAuditResult::default();
        };
        // check フェーズと同じレート制限状態を使う。別インスタンスにすると
        // 直前のリクエストから 1 秒経たずに監査の 1 発目が飛ぶ
        let adapter = CratesIoAdapter::with_rate_limit(
            self.client.clone(),
            self.crates_io_rate_limit.clone(),
        );
        let audit = LockAgeAudit {
            project_dir,
            cutoff,
            exemptions: &policy.exemptions,
            baseline,
            preferred,
            adapter: &adapter,
            cargo: &CargoCommand::new(),
            budget: LOCK_AGE_AUDIT_BUDGET,
        };
        self.audit_lock_age(&audit, bar).await
    }

    /// `enforce_lock_age_rust` の本体。公開日の取得元・cargo の起動設定・基準日時を
    /// 差し替えられるように分けてある (テストで crates.io と実行日に依存しないため)
    async fn audit_lock_age(
        &self,
        audit: &LockAgeAudit<'_>,
        bar: Option<&ProgressBar>,
    ) -> LockAgeAuditResult {
        let LockAgeAudit {
            project_dir,
            cutoff,
            exemptions,
            baseline,
            preferred,
            adapter,
            cargo,
            budget,
        } = *audit;
        let lock_path = project_dir.join("Cargo.lock");
        let mut log = AdjustmentLog::default();
        // 1 件ずつの `--precise` を試した組合せ。成否によらず 1 件ずつは再試行しない。
        // resolver 制約で失敗した組合せを 1 件ずつ試し直すと、パスごとに同じ
        // `cargo update` を繰り返して (失敗件数 × 最大 5 パス) 分だけ監査が伸びる
        let mut tried_alone: HashSet<(String, String)> = HashSet::new();
        // 1 件ずつでは衝突して戻せなかった組合せと、そのときの cargo のエラー。
        // lock が変われば (別の差し戻しで制約が緩めば) まとめ解きで試し直す
        let mut conflicts: HashMap<(String, String), String> = HashMap::new();
        // まとめ解きが 1 件も戻せなかったときの lock の状態。同じ状態で同じ解き直しを
        // 繰り返さない (状態が変われば試し直す)
        let mut failed_together: HashSet<u64> = HashSet::new();
        // まとめ解きで起きた、利用者に必ず知らせるべき問題
        let mut problems: Vec<String> = Vec::new();
        let started = Instant::now();
        let deadline = started + budget;

        // 上限回数まで差し戻した後の 1 回は、差し戻しをせずに残った違反を数えるだけに使う。
        // これが無いと、最後のパスの差し戻しで新しく入った版が監査されないまま終わる
        for pass in 0..=MAX_ENFORCE_LOCK_AGE_PASSES {
            let verify_only = pass == MAX_ENFORCE_LOCK_AGE_PASSES;
            let lock_content = std::fs::read_to_string(&lock_path).unwrap_or_default();
            let entries = parse_registry_entries(&lock_content);
            // install 前と同じバージョンで lock されていた依存は depup の更新で入った
            // ものではないため監査しない。crates.io は 1 リクエスト/秒に直列化される
            // ので、lock 全体 (数百件) を舐めると監査だけで数分の無音待ちになる。
            let targets = changed_entries(&entries, baseline);
            if targets.is_empty() {
                break;
            }
            if let Some(b) = bar {
                // 位置を先に戻す。長さを先に縮めると、前パスの位置が残っている間に
                // 描画されて `14/1` のような不整合が一瞬見える
                b.set_position(0);
                b.set_length(targets.len() as u64);
            }
            if pass == 0 && self.args.verbose {
                let message = format!(
                    "  {} — {} crate(s) changed by install; checking release dates",
                    project_dir.display(),
                    targets.len()
                );
                match bar {
                    Some(b) => b.suspend(|| eprintln!("{message}")),
                    None => eprintln!("{message}"),
                }
            }

            let mut together: Vec<RollbackCandidate> = Vec::new();
            let mut any_downgraded = false;
            let mut budget_exhausted = false;
            let mut completed = 0usize;

            'audit: for (index, (name, versions)) in targets.iter().enumerate() {
                let elapsed = started.elapsed();
                if elapsed >= budget {
                    // 予算切れ: 監査済みの調整は活かしつつ打ち切る。確かめられなかった件数は
                    // 最後に最終の lock から数える
                    budget_exhausted = true;
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
                    tokio::time::timeout(budget - elapsed, self.fetch_versions(adapter, name))
                        .await;
                let Ok(fetched) = fetched else {
                    budget_exhausted = true;
                    break;
                };
                let all_versions = match fetched {
                    Ok(v) => v,
                    Err(_) => {
                        for v in versions {
                            log.record_if_absent(
                                name,
                                v,
                                None,
                                LockAgeStatus::ReleaseDateUnavailable,
                            );
                        }
                        // fetch 失敗でもこの対象は「処理済み」。ここで進捗を進めないと
                        // 末尾の数件が失敗したときにバーが手前で止まったまま次の
                        // ディレクトリへ移る (全件失敗なら 0 のまま)
                        completed = index + 1;
                        continue;
                    }
                };

                for current in versions {
                    let key = (name.clone(), current.clone());
                    let Some(current_info) = all_versions.iter().find(|v| {
                        compare_versions(&v.version, current) == std::cmp::Ordering::Equal
                    }) else {
                        continue;
                    };

                    if exemptions.admits(Language::Rust, name, current_info, cutoff) {
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
                        log.record(name, current, None, LockAgeStatus::NoOlderCandidate);
                        continue;
                    };
                    let candidate = RollbackCandidate {
                        name: name.clone(),
                        current: current.clone(),
                        target: target.clone(),
                        minimum: version_floor(baseline.get(name).map(Vec::as_slice), current),
                    };

                    if verify_only {
                        // 差し戻しの結果がまだ付いていない違反だけを「上限到達」にする
                        // (1 件ずつ・まとめ解きで失敗した理由は上書きしない)
                        log.record_if_absent(
                            name,
                            current,
                            None,
                            LockAgeStatus::NotAttempted(format!(
                                "stopped after {MAX_ENFORCE_LOCK_AGE_PASSES} rounds of rollbacks"
                            )),
                        );
                        continue;
                    }

                    if tried_alone.contains(&key) {
                        // 1 件ずつは試し済み。衝突で失敗したものは、まとめ解きの対象に戻す
                        // (lock が前回のまとめ解きの失敗時から変わっていれば試し直される)
                        if conflicts.contains_key(&key) {
                            together.push(candidate);
                        }
                        continue;
                    }

                    // 残り予算が `cargo update` 1 回分に満たないなら着手しない。
                    // 1 件ずつの試行は二度と再試行されないため、1 秒程度に切り詰められた
                    // タイムアウトで走らせると、本来差し戻せた依存が「失敗」として恒久的に
                    // 確定してしまう。未検証として残す方が次回の実行で救える。
                    // まとめ解きを待っている crate があるときは、その分の時間も残す
                    // (1 件ずつの試行だけで予算を使い切らない)
                    let remaining = budget.saturating_sub(started.elapsed());
                    let reserve = if together.is_empty() {
                        Duration::ZERO
                    } else {
                        MIN_TOGETHER_SLICE
                    };
                    if remaining < MIN_CARGO_UPDATE_SLICE + reserve {
                        if !together.is_empty() && remaining >= MIN_TOGETHER_SLICE {
                            // 1 件ずつ試す時間は無いが、まとめ解きの枠は残っている
                            together.push(candidate);
                            continue;
                        }
                        budget_exhausted = true;
                        break 'audit;
                    }

                    tried_alone.insert(key.clone());
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
                        &target,
                        remaining.min(CARGO_UPDATE_TIMEOUT),
                    )
                    .await;
                    // cargo が成功を返しても lock が変わったとは限らない (directory source
                    // への置き換えでは `--precise` が何もせずに exit 0 で終わる)。lock を
                    // 読み直し、実際に今の版から外れたことを確かめてから差し戻しとして数える
                    let status = match status {
                        LockAgeStatus::Downgraded => {
                            let locked = parse_registry_entries(
                                &std::fs::read_to_string(&lock_path).unwrap_or_default(),
                            );
                            match locked_after_rollback(&locked, name, current) {
                                Some(moved) => {
                                    any_downgraded = true;
                                    let status = if moved.is_some() {
                                        LockAgeStatus::Downgraded
                                    } else {
                                        LockAgeStatus::Removed
                                    };
                                    log.record(name, current, moved, status);
                                    continue;
                                }
                                None => LockAgeStatus::UpdateCommandFailed(format!(
                                    "cargo update --precise {target} exited successfully, but Cargo.lock still has {name} {current}"
                                )),
                            }
                        }
                        other => other,
                    };
                    match status {
                        LockAgeStatus::UpdateCommandFailed(message) => {
                            conflicts.insert(key, message.clone());
                            together.push(candidate);
                            log.record(
                                name,
                                current,
                                None,
                                LockAgeStatus::UpdateCommandFailed(message),
                            );
                        }
                        other => log.record(name, current, None, other),
                    }
                }
                completed = index + 1;
            }

            // 1 件ずつでは衝突して戻せなかった crate を、まとめて解き直す
            if !together.is_empty() && !verify_only {
                // 同じパスの 1 件ずつの差し戻しで lock は変わっているので、直前の状態で判定する
                let state = together_fingerprint(
                    &std::fs::read_to_string(&lock_path).unwrap_or_default(),
                    &together,
                );
                let remaining = budget.saturating_sub(started.elapsed());
                if remaining < MIN_TOGETHER_SLICE {
                    for candidate in &together {
                        log.record_if_absent(
                            &candidate.name,
                            &candidate.current,
                            None,
                            LockAgeStatus::NotAttempted(
                                "audit time budget ran out before resolving together".to_string(),
                            ),
                        );
                    }
                    budget_exhausted = true;
                } else if !failed_together.contains(&state) {
                    if let Some(b) = bar {
                        b.set_message(format!("Resolving {} crate(s) together", together.len()));
                    }
                    let report = resolve_together(
                        project_dir,
                        &together,
                        cargo,
                        deadline,
                        CARGO_UPDATE_TIMEOUT,
                    )
                    .await;
                    problems.extend(report.problems);
                    let mut resolved_any = false;
                    for (candidate, outcome) in report.outcomes {
                        let key = (candidate.name.clone(), candidate.current.clone());
                        match outcome {
                            TogetherOutcome::Resolved(version) => {
                                resolved_any = true;
                                conflicts.remove(&key);
                                log.record(
                                    &candidate.name,
                                    &candidate.current,
                                    Some(version),
                                    LockAgeStatus::Downgraded,
                                );
                            }
                            TogetherOutcome::Removed => {
                                resolved_any = true;
                                conflicts.remove(&key);
                                log.record(
                                    &candidate.name,
                                    &candidate.current,
                                    None,
                                    LockAgeStatus::Removed,
                                );
                            }
                            TogetherOutcome::BlockedByManifest(requirement) => {
                                conflicts.remove(&key);
                                log.record(
                                    &candidate.name,
                                    &candidate.current,
                                    None,
                                    LockAgeStatus::BlockedByManifest(requirement),
                                );
                            }
                            TogetherOutcome::Failed(reason) => {
                                let message = match conflicts.get(&key) {
                                    Some(alone) => {
                                        format!(
                                            "{alone}\n(resolving together also failed: {reason})"
                                        )
                                    }
                                    None => format!("resolving together failed: {reason}"),
                                };
                                log.record(
                                    &candidate.name,
                                    &candidate.current,
                                    None,
                                    LockAgeStatus::UpdateCommandFailed(message),
                                );
                            }
                            // 時間切れは cargo の失敗ではない。1 件ずつで衝突した理由があれば
                            // それを残し、無ければ試さなかったことを記録する
                            TogetherOutcome::NotAttempted(reason) => {
                                let status = match conflicts.get(&key) {
                                    Some(alone) => LockAgeStatus::UpdateCommandFailed(format!(
                                        "{alone}\n(resolving together was not finished: {reason})"
                                    )),
                                    None => LockAgeStatus::NotAttempted(reason),
                                };
                                log.record(&candidate.name, &candidate.current, None, status);
                            }
                        }
                    }
                    if resolved_any {
                        any_downgraded = true;
                    } else {
                        failed_together.insert(state);
                    }
                } else {
                    // 同じ lock の状態で既に失敗している。1 件ずつも試していない
                    // (予算の都合でまとめ解きに回した) crate が報告から漏れないようにする
                    for candidate in &together {
                        log.record_if_absent(
                            &candidate.name,
                            &candidate.current,
                            None,
                            LockAgeStatus::NotAttempted(
                                "resolving together already failed for the same Cargo.lock"
                                    .to_string(),
                            ),
                        );
                    }
                }
            }

            // 予算切れで抜けた場合は実際に監査できた位置で止める (完走に見せない)
            if let Some(b) = bar {
                b.set_position(completed as u64);
            }

            if budget_exhausted || verify_only {
                break;
            }

            if !any_downgraded {
                // このパスで実際の差し戻しが発生しなかった → 収束
                break;
            }
        }

        // 報告は最終的な Cargo.lock の状態から組み立て直す。途中の記録だけで「差し戻した」と
        // 言うと、後の差し戻しで版が動いた・まとめ解きの結果を確かめる前に予算が尽きた等の
        // 場合に、lock の実態と食い違う
        let final_entries =
            parse_registry_entries(&std::fs::read_to_string(&lock_path).unwrap_or_default());
        log.reconcile_with_lock(&final_entries);
        // install 前の版へ戻しただけで、その版も期間を満たさないものは「期間を満たすよう
        // 差し戻した」と区別する (供給網対策として --age を使う利用者に、満たしたと読ませない)
        let returned: Vec<(String, String, String)> = log
            .entries
            .values()
            .filter(|adjustment| adjustment.status == LockAgeStatus::Downgraded)
            .filter_map(|adjustment| {
                let to = adjustment.to.as_ref()?;
                let locked_before = baseline
                    .get(&adjustment.name)
                    .is_some_and(|versions| versions.contains(to));
                locked_before
                    .then(|| (adjustment.name.clone(), adjustment.from.clone(), to.clone()))
            })
            .collect();
        for (name, from, to) in returned {
            let known = self.cached_versions(adapter, &name).await;
            let young = known.is_some_and(|all| {
                all.iter().any(|info| {
                    compare_versions(&info.version, &to) == std::cmp::Ordering::Equal
                        && !exemptions.admits(Language::Rust, &name, info, cutoff)
                })
            });
            if young {
                log.record(&name, &from, Some(to), LockAgeStatus::Restored);
            }
        }
        let mut unchecked = 0usize;
        for (name, versions) in changed_entries(&final_entries, baseline) {
            // ここではレジストリへ問い合わせない (予算を使い切っていることがある)。
            // 監査中に取得した公開日だけで判定し、取得していないものは未検証として数える
            let known = self.cached_versions(adapter, &name).await;
            for version in versions {
                if log.contains(&name, &version) {
                    continue;
                }
                let info = known.as_ref().and_then(|all| {
                    all.iter().find(|info| {
                        compare_versions(&info.version, &version) == std::cmp::Ordering::Equal
                    })
                });
                match (known.as_ref(), info) {
                    (Some(_), Some(info))
                        if !exemptions.admits(Language::Rust, &name, info, cutoff) =>
                    {
                        log.record(
                            &name,
                            &version,
                            None,
                            LockAgeStatus::NotAttempted(
                                "the audit ended before rolling it back".to_string(),
                            ),
                        )
                    }
                    // 期間を満たす版、または crates.io の一覧に無い版 (yank 済みなど) は
                    // 監査中と同じく違反として扱わない
                    (Some(_), _) => {}
                    (None, _) => unchecked += 1,
                }
            }
        }

        LockAgeAuditResult {
            adjustments: log.into_adjustments(),
            unchecked,
            problems,
        }
    }

    /// 監査中に取得済みの版一覧 (レジストリへは問い合わせない)
    async fn cached_versions(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> Option<Vec<VersionInfo>> {
        let cache = self.version_cache.lock().await;
        cache
            .get(&(adapter.language(), package.to_string()))
            .cloned()
    }

    /// 1 つの依存を処理する: 早期スキップ判定 → fetch → OSV チェック → judge。
    ///
    /// `bar` が `Some` のとき、OSV チェック開始時に進捗メッセージを更新する。
    async fn process_one_dependency(
        &self,
        dep: Dependency,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        judge: &UpdateJudge,
        bar: Option<&ProgressBar>,
    ) -> OnePassResult {
        // 早期スキップ判定
        if let Some(reason) = judge.should_skip(&dep) {
            return OnePassResult {
                name: dep.name.clone(),
                outcome: UpdateResult::skip(dep, reason),
                fetch_error: None,
                osv_warnings: Vec::new(),
            };
        }

        // git 依存は専用ロジック
        if dep.is_git() {
            let result = self.judge_git_dependency(&dep).await;
            return OnePassResult {
                name: dep.name.clone(),
                outcome: result,
                fetch_error: None,
                osv_warnings: Vec::new(),
            };
        }

        // registry 経由のフェッチ
        match self.fetch_versions(adapter, &dep.name).await {
            Ok(versions) => {
                // OSV チェック: judge で採用しようとした候補だけを問い合わせる。
                // 脆弱なら、その候補を除外して再 judge するループで安全な候補に
                // 自然にフォールバックする (1 依存あたり通常 1〜2 API call で済む)。
                // Swift など osv_ecosystem() == None の言語はスキップ。
                let mut osv_warnings = Vec::new();
                let result = match (self.osv_checker.as_ref(), dep.language.osv_ecosystem()) {
                    (Some(checker), Some(eco)) => {
                        judge_with_osv(judge, &dep, versions, checker, eco, bar, &mut osv_warnings)
                            .await
                    }
                    _ => judge.judge(&dep, &versions),
                };
                OnePassResult {
                    name: dep.name.clone(),
                    outcome: result,
                    fetch_error: None,
                    osv_warnings,
                }
            }
            Err(e) => {
                let err_msg = e.clone();
                OnePassResult {
                    name: dep.name.clone(),
                    outcome: UpdateResult::skip(dep, SkipReason::FetchFailed(e)),
                    fetch_error: Some(err_msg),
                    osv_warnings: Vec::new(),
                }
            }
        }
    }

    /// git 依存の判定を実行する
    ///
    /// - branch / DefaultBranch: リモート HEAD/ブランチ commit と現在 commit を比較し、新しければ更新
    /// - tag: リモートの全タグから最新 semver を選び、現在の tag より新しければ更新
    /// - rev: 常にスキップ (pinned 扱い。`--include-pinned` でも更新しない)
    async fn judge_git_dependency(&self, dep: &Dependency) -> UpdateResult {
        let Some(git) = dep.git_source.as_ref() else {
            return UpdateResult::skip(
                dep.clone(),
                SkipReason::ParseError("missing git source".to_string()),
            );
        };

        let refs = match self.git_remote.fetch(&git.url).await {
            Ok(refs) => refs,
            Err(e) => {
                return UpdateResult::skip_fetch_failed(dep.clone(), e.to_string());
            }
        };

        match &git.reference {
            GitReference::Branch(branch) => {
                let Some(latest) = refs.branch_commit(branch) else {
                    return UpdateResult::skip(
                        dep.clone(),
                        SkipReason::FetchFailed(format!("branch '{}' not found on remote", branch)),
                    );
                };
                compare_and_update_commit(dep, latest, git.current_commit.as_deref())
            }
            GitReference::DefaultBranch => {
                let Some(latest) = refs.head_commit() else {
                    return UpdateResult::skip(
                        dep.clone(),
                        SkipReason::FetchFailed("remote HEAD not found".to_string()),
                    );
                };
                compare_and_update_commit(dep, latest, git.current_commit.as_deref())
            }
            GitReference::Rev(_) => {
                // rev 固定は Cargo.toml の rev 書き換えが必要なため現時点では常にスキップ。
                // --include-pinned 指定時もサポート対象外 (情報提供のみ別経路で検討)。
                UpdateResult::skip_pinned(dep.clone())
            }
            GitReference::Tag(current_tag) => {
                let tags = refs.all_tag_names();
                let Some(latest_tag) = latest_semver_tag(&tags) else {
                    return UpdateResult::skip(dep.clone(), SkipReason::NoSuitableVersion);
                };
                if compare_versions(&latest_tag, current_tag) != std::cmp::Ordering::Greater {
                    return UpdateResult::skip_already_latest(dep.clone());
                }
                // git tag 依存はマニフェストの tag 文字列を実際に書き換えるため、
                // レジストリ依存と同じく `--max-change` を尊重する。git 依存は
                // `UpdateJudge::judge` を通らないので、ここで明示的に適用しないと
                // `--max-change patch` でも major タグへ更新されてしまう。
                // 判定不能なタグは `apply_max_change_filter` と同じく通す。
                if let Some(max) = self.args.max_change
                    && crate::domain::ChangeLevel::from_versions(current_tag, &latest_tag)
                        .is_some_and(|level| level > max)
                {
                    return UpdateResult::skip(dep.clone(), SkipReason::ChangeLevelLimited(max));
                }
                UpdateResult::update(dep.clone(), latest_tag)
            }
        }
    }

    /// 更新ワークフローを実行する
    pub async fn run(&self) -> OrchestratorResult {
        self.run_with_progress(!self.args.quiet).await
    }

    /// プログレス表示オプション付きで更新ワークフローを実行する
    pub async fn run_with_progress(&self, show_progress: bool) -> OrchestratorResult {
        let mut progress = Progress::new(show_progress);

        // ステップ1: マニフェストファイルを検出
        progress.spinner("Detecting manifest files...");
        let manifests = detect_manifests(&self.args.path);
        progress.finish_and_clear();

        self.process_manifests(&manifests, &mut progress).await
    }

    /// 複数ディレクトリにまたがって更新ワークフローを実行する (モノレポモード)
    ///
    /// 各ディレクトリのマニフェストを検出し、バージョンキャッシュを共有して
    /// 統合された結果を生成する。
    pub async fn run_directories(&self, directories: &[PathBuf]) -> OrchestratorResult {
        let mut progress = Progress::new(!self.args.quiet);

        // ステップ1: 全ディレクトリのマニフェストファイルを検出。
        // `.depup` にルートとサブディレクトリが両方含まれる場合、ルート側の
        // workspace 自動検出とサブディレクトリ側の検出で同一マニフェストが
        // 重複するため、正規化パスで重複排除する。
        progress.spinner("Detecting manifest files...");
        let mut all_manifests: Vec<ManifestInfo> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for dir in directories {
            for manifest in detect_manifests(dir) {
                let key =
                    std::fs::canonicalize(&manifest.path).unwrap_or_else(|_| manifest.path.clone());
                if seen.insert(key) {
                    all_manifests.push(manifest);
                }
            }
        }
        progress.finish_and_clear();

        self.process_manifests(&all_manifests, &mut progress).await
    }

    /// 検出されたマニフェストを処理: パース、バージョン取得、更新判定、結果書き込み
    async fn process_manifests(
        &self,
        manifests: &[ManifestInfo],
        progress: &mut Progress,
    ) -> OrchestratorResult {
        let mut summary = UpdateSummary::new(self.args.dry_run);
        let mut errors = Vec::new();

        if manifests.is_empty() {
            return OrchestratorResult {
                summary,
                write_results: Vec::new(),
                errors,
            };
        }

        // mise のバージョン解決は `mise` コマンドに委譲するため、未インストールなら
        // 依存ごとに同じ fetch エラーを並べる前にマニフェストごと外す。
        let manifests = self.filter_out_unusable_mise_manifests(manifests);
        let manifests = manifests.as_slice();

        let parsed = self.parse_phase(manifests, progress, &mut errors);
        self.check_phase(parsed, progress, &mut summary, &mut errors)
            .await;
        self.sync_tauri_if_needed(manifests, progress, &mut summary, &mut errors)
            .await;
        let write_results = self.write_phase(&summary, progress, &mut errors);

        OrchestratorResult {
            summary,
            write_results,
            errors,
        }
    }

    /// パース phase: 各マニフェストを読み、依存配列を作る。
    /// 言語フィルタに該当しないマニフェストはスキップ、読み込み/パースエラーは `errors` に追加して継続する。
    /// Rust プロジェクトでは Cargo.lock から git 依存の現在コミットも補完する。
    fn parse_phase<'a>(
        &self,
        manifests: &'a [ManifestInfo],
        progress: &mut Progress,
        errors: &mut Vec<OrchestratorError>,
    ) -> Vec<ParsedManifest<'a>> {
        progress.spinner("Parsing manifests...");
        let mut parsed = Vec::new();
        for info in manifests {
            if !self.should_process_language(info.language) {
                continue;
            }
            let parser = get_parser(info.language);
            let parse_error = |message: String| OrchestratorError::ManifestParseError {
                path: info.path.display().to_string(),
                message,
            };
            let content = match std::fs::read_to_string(&info.path) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(parse_error(e.to_string()));
                    continue;
                }
            };
            let mut dependencies = match parser.parse(&content) {
                Ok(deps) => deps,
                Err(e) => {
                    errors.push(parse_error(e.to_string()));
                    continue;
                }
            };
            if info.language == Language::Rust {
                enrich_with_cargo_lock(&info.path, &self.rust_lock_boundary(), &mut dependencies);
            }
            parsed.push(ParsedManifest { info, dependencies });
        }
        progress.finish_and_clear();
        parsed
    }

    /// チェック phase: 各依存のバージョンを並列取得し、`judge` で判定して `summary` に集約する。
    async fn check_phase<'a>(
        &self,
        parsed: Vec<ParsedManifest<'a>>,
        progress: &mut Progress,
        summary: &mut UpdateSummary,
        errors: &mut Vec<OrchestratorError>,
    ) {
        let total_deps: usize = parsed.iter().map(|p| p.dependencies.len()).sum();
        progress.start(total_deps as u64, "Checking dependencies");
        let progress_bar = progress.bar();

        for ParsedManifest { info, dependencies } in parsed {
            let dir = info.path.parent().unwrap_or(&self.args.path);
            let mut filter = self.build_filter_for(dir);
            let policy = match self.resolved_age_policy_for_language(dir, info.language) {
                Ok(policy) => policy,
                Err(message) => {
                    errors.push(OrchestratorError::ManifestParseError {
                        path: info.path.display().to_string(),
                        message,
                    });
                    continue;
                }
            };
            filter.min_age = policy.min_age;
            filter.age_exempt = policy.exemptions.clone();
            if filter.min_age.is_some()
                && !filter.age_exempt.is_empty()
                && !dependencies.is_empty()
                && !matches!(
                    info.language,
                    Language::Rust | Language::Go | Language::Swift
                )
            {
                eprintln!(
                    "Notice: age_exempt.github is unavailable for other registries; their dependencies retain the age filter"
                );
            }
            let judge = UpdateJudge::with_time(filter, self.evaluated_at);
            let judge = &judge;
            let mut manifest_result = ManifestUpdateResult::new(&info.path, info.language);
            // 複数 future から共有するため Arc に変換
            let adapter: Arc<dyn RegistryAdapter + Send + Sync> = if info.language == Language::Mise
            {
                Arc::new(MiseAdapter::for_project(dir, policy.cutoff()))
            } else {
                Arc::from(self.get_adapter(info.language))
            };

            // 依存数に応じて並列度を調整 (1〜4)。
            // 結果は入力順で返るため出力順は安定する (`buffered`: ordered)。
            // fetch_versions は内部でレジストリ別の Semaphore を持つため、
            // crates.io のレート制限などは従来どおり尊重される。
            // 各タスクの開始/OSV 開始/完了で `ProgressBar` を直接更新するため、
            // collect 前から `pos` と `msg` が動く。
            let concurrency = version_check_concurrency(dependencies.len());
            let results: Vec<OnePassResult> = stream::iter(dependencies)
                .map(|dep| {
                    let adapter = Arc::clone(&adapter);
                    let bar = progress_bar.clone();
                    async move {
                        if let Some(ref b) = bar {
                            b.set_message(format!("Checking {}", dep.name));
                        }
                        let result = self
                            .process_one_dependency(dep, &*adapter, judge, bar.as_ref())
                            .await;
                        if let Some(ref b) = bar {
                            b.inc(1);
                        }
                        result
                    }
                })
                .buffered(concurrency)
                .collect()
                .await;

            // errors / manifest_result を順序を保って集約 (progress は並列タスク側で更新済み)
            for result in results {
                if let Some(err_msg) = result.fetch_error {
                    errors.push(OrchestratorError::RegistryError {
                        package: result.name.clone(),
                        message: err_msg,
                    });
                }
                for warn in result.osv_warnings {
                    errors.push(OrchestratorError::OsvWarning {
                        package: result.name.clone(),
                        message: warn,
                    });
                }
                manifest_result.add_result(result.outcome);
            }
            summary.add_manifest(manifest_result);
        }
        progress.finish_and_clear();
    }

    /// Tauri プロジェクトが含まれていれば、npm / crate のバージョンを同期する。
    async fn sync_tauri_if_needed(
        &self,
        manifests: &[ManifestInfo],
        progress: &mut Progress,
        summary: &mut UpdateSummary,
        errors: &mut Vec<OrchestratorError>,
    ) {
        if !manifests.iter().any(|m| m.is_tauri_rust) {
            return;
        }
        progress.spinner("Synchronizing Tauri versions...");
        for manifest in manifests.iter().filter(|manifest| manifest.is_tauri_rust) {
            let Some(project_dir) = manifest.path.parent().and_then(Path::parent) else {
                continue;
            };
            self.synchronize_tauri_versions(summary, errors, project_dir)
                .await;
        }
        progress.finish_and_clear();
    }

    /// 書き込み phase: 更新を適用 (`dry_run` ならプレビューのみ) し、書き込みエラーを集約する。
    fn write_phase(
        &self,
        summary: &UpdateSummary,
        progress: &mut Progress,
        errors: &mut Vec<OrchestratorError>,
    ) -> Vec<WriteResult> {
        if !self.args.dry_run {
            progress.spinner("Writing updates...");
        }
        let writer = ManifestWriter::new(self.args.dry_run);
        let write_results = writer.apply_all_updates(&summary.manifests, get_parser);
        progress.finish_and_clear();

        for result in &write_results {
            for error in &result.errors {
                errors.push(OrchestratorError::WriteError {
                    path: result.path.display().to_string(),
                    message: error.clone(),
                });
            }
        }
        write_results
    }

    /// CLI引数からUpdateFilterを構築する
    #[cfg(test)]
    fn build_filter(&self) -> UpdateFilter {
        self.build_filter_for(&self.args.path)
    }

    fn build_filter_for(&self, dir: &Path) -> UpdateFilter {
        let mut filter = UpdateFilter::new();

        // 言語フィルタ
        let selected = self.args.selected_languages();
        if !selected.is_empty() {
            filter = filter.with_languages(selected);
        }

        // パッケージフィルタ
        if !self.args.exclude.is_empty() {
            filter = filter.with_exclude(self.args.exclude.clone());
        }
        if !self.args.only.is_empty() {
            filter = filter.with_only(self.args.only.clone());
        }

        // ピン留めバージョンを含める
        if self.args.include_pinned {
            filter = filter.with_include_pinned(true);
        }

        let resolved = age::resolve(
            &self.args,
            self.global_config.as_ref(),
            dir,
            self.evaluated_at,
        );
        resolved.emit_notice();
        filter.min_age = resolved.policy.min_age;
        filter.age_exempt = resolved.policy.exemptions;
        // mise の除外設定は depup 側で解釈しないため、食い違いを通知する
        self.warn_mise_age_excludes();

        // 変更レベル上限
        if let Some(level) = self.args.max_change {
            filter = filter.with_max_change(level);
        }

        filter
    }

    /// ディレクトリの明示設定と実行ルート内の祖先設定から age を解決する。
    pub fn resolved_age_policy_for(&self, dir: &Path) -> AgePolicy {
        age::resolve(
            &self.args,
            self.global_config.as_ref(),
            dir,
            self.evaluated_at,
        )
        .policy
    }

    /// PM 自身の明示設定を含め、より厳しい値を維持する。
    pub fn resolved_age_policy_for_language(
        &self,
        dir: &Path,
        language: Language,
    ) -> Result<AgePolicy, String> {
        let mut policy = self.resolved_age_policy_for(dir);
        if let Some(age) =
            crate::manifest::native_age::native_min_age(language, dir, self.evaluated_at)?
        {
            policy.min_age = policy.min_age.max(Some(age));
            policy.exemptions = crate::update::AgeExemptions::default();
        }
        Ok(policy)
    }

    /// member 内で起動した場合も、cargo が実際に更新する workspace lock を監査する。
    pub fn rust_lock_boundary(&self) -> PathBuf {
        age::cargo_workspace_root(&self.args.path)
    }

    /// 実行ルートの age (既存のライブラリ API)。
    pub fn resolved_min_age(&self) -> Option<Duration> {
        self.resolved_age_policy_for(&self.args.path).min_age
    }

    #[cfg(test)]
    fn resolved_age_exemptions(&self) -> crate::update::AgeExemptions {
        self.resolved_age_policy_for(&self.args.path).exemptions
    }

    /// `mise` コマンドが無い環境では mise マニフェストを処理対象から外す。
    ///
    /// mise のバージョン解決は `mise ls-remote` に委譲しているため、コマンドが
    /// 無ければ全ツールが同じ理由で fetch 失敗する。依存の数だけ同じエラーを
    /// 並べても情報量がないので、マニフェスト単位で外して警告を 1 回だけ出す。
    fn filter_out_unusable_mise_manifests(&self, manifests: &[ManifestInfo]) -> Vec<ManifestInfo> {
        use colored::Colorize as _;

        let mise_manifests = manifests
            .iter()
            .filter(|m| m.language == Language::Mise)
            .count();
        if mise_manifests == 0 || MiseAdapter::is_available() {
            return manifests.to_vec();
        }

        let msg = format!(
            "⚠ skipping {} mise manifest(s): `mise` command not found in PATH (see https://mise.jdx.dev)",
            mise_manifests
        );
        eprintln!("{}", msg.yellow());

        manifests
            .iter()
            .filter(|m| m.language != Language::Mise)
            .cloned()
            .collect()
    }

    /// mise の `minimum_release_age_excludes` は depup 側では解釈しないため、
    /// 設定されている場合に一度だけ注意を促す。
    ///
    /// mise は excludes に挙げたツールを age 制約から外すが、depup は
    /// 全ツールへ一律に age を適用する。両者の食い違い (depup では「更新なし」
    /// なのに `mise install` では新しい版が入る、など) を黙って起こさないよう、
    /// 差があること自体を伝える。
    fn warn_mise_age_excludes(&self) {
        use colored::Colorize as _;
        if !has_mise_config(&self.args.path) {
            return;
        }
        let mise = MiseSettings::from_dir(&self.args.path);
        if mise.minimum_release_age_excludes.is_empty() {
            return;
        }
        let msg = format!(
            "⚠ mise's minimum_release_age_excludes ({}) is not applied by depup: age is enforced for all tools",
            mise.minimum_release_age_excludes.join(", ")
        );
        eprintln!("{}", msg.yellow());
    }

    /// CLI引数に基づいて言語を処理すべきかチェックする
    fn should_process_language(&self, language: Language) -> bool {
        if !self.args.has_language_filter() {
            return true;
        }
        match language {
            Language::Node => self.args.node,
            Language::Python => self.args.python,
            Language::Rust => self.args.rust_lang,
            Language::Go => self.args.go,
            Language::Ruby => self.args.ruby,
            Language::Php => self.args.php,
            Language::Java => self.args.java,
            Language::Swift => self.args.swift,
            Language::Mise => self.args.mise,
        }
    }

    /// 言語に対応するレジストリアダプタを取得する
    fn get_adapter(&self, language: Language) -> Box<dyn RegistryAdapter + Send + Sync> {
        match language {
            Language::Node => Box::new(NpmAdapter::new(self.client.clone())),
            Language::Python => Box::new(PyPIAdapter::new(self.client.clone())),
            Language::Rust => Box::new(CratesIoAdapter::with_rate_limit(
                self.client.clone(),
                self.crates_io_rate_limit.clone(),
            )),
            Language::Go => Box::new(GoProxyAdapter::new(self.client.clone())),
            Language::Ruby => Box::new(RubyGemsAdapter::new(self.client.clone())),
            Language::Php => Box::new(PackagistAdapter::new(self.client.clone())),
            Language::Java => Box::new(MavenCentralAdapter::new(self.client.clone())),
            Language::Swift => Box::new(GitHubTagsAdapter::new(self.client.clone())),
            // mise は HTTP レジストリではなく `mise ls-remote` の呼び出しで解決する
            Language::Mise => Box::new(MiseAdapter::new()),
        }
    }

    /// 同時実行制御とキャッシュ付きでレジストリからバージョンを取得する
    async fn fetch_versions(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> Result<Vec<VersionInfo>, String> {
        // mise は現在ディレクトリと native age により一覧自体が変わる。
        // その一覧を別の age スコープへ再利用しない (mise 自身のリモートキャッシュは使える)。
        if adapter.language() == Language::Mise {
            let _permit = self.general_semaphore.acquire().await.unwrap();
            return adapter
                .fetch_versions(package)
                .await
                .map_err(|error| error.to_string());
        }
        let cache_key = (adapter.language(), package.to_string());

        // まずキャッシュを確認
        {
            let cache = self.version_cache.lock().await;
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

        // 同一キーの取得だけを直列化する。異なるパッケージの並列性は維持する。
        let fetch_lock = {
            let mut locks = self.version_fetch_locks.lock().await;
            Arc::clone(
                locks
                    .entry(cache_key.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _fetch_guard = fetch_lock.lock().await;

        // キー単位ロックの待機中に先行取得が完了している場合はキャッシュを返す。
        {
            let cache = self.version_cache.lock().await;
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

        // レジストリに応じて適切なセマフォを使用
        let semaphore = if adapter.language() == Language::Rust {
            &self.crates_io_semaphore
        } else {
            &self.general_semaphore
        };

        let _permit = semaphore.acquire().await.unwrap();

        let result = adapter
            .fetch_versions(package)
            .await
            .map_err(|e| e.to_string())?;

        // キャッシュに保存
        {
            let mut cache = self.version_cache.lock().await;
            cache.insert(cache_key, result.clone());
        }

        Ok(result)
    }

    /// Tauriパッケージバージョンを同期する (@tauri-apps/api, @tauri-apps/cli, tauri crate)
    ///
    /// Tauriビルドエラーを防ぐため、全パッケージのメジャー.マイナーバージョンを
    /// 一致させる。
    async fn synchronize_tauri_versions(
        &self,
        summary: &mut UpdateSummary,
        errors: &mut Vec<OrchestratorError>,
        project_dir: &Path,
    ) {
        // Nodeマニフェスト内の全Tauri npmパッケージを検索
        // 戻り値: Vec<(manifest_idx, result_idx, result, current_version)>
        let npm_packages: Vec<(usize, usize, UpdateResult, String)> =
            collect_tauri_packages(summary, project_dir, Language::Node, |name| {
                TAURI_NPM_PACKAGES.contains(&name)
            });

        // Rustマニフェスト内のtauri crateを検索
        let crate_info: Option<(usize, usize, UpdateResult, String)> =
            collect_tauri_packages(summary, project_dir, Language::Rust, |name| {
                name == TAURI_CRATE
            })
            .into_iter()
            .next();

        // tauriパッケージが一つも見つからなければ、同期不要
        if npm_packages.is_empty() && crate_info.is_none() {
            return;
        }

        // ユーザの明示的なフィルタや判定不能 (--exclude / --only / 言語フィルタ /
        // pinned / --max-change / fetch・parse 失敗) でスキップされた側があるときは
        // 同期しない。judge のフィルタ決定を同期が上書きして書き込むのは利用者の
        // 意図に反するため (AlreadyLatest / NoSuitableVersion のみ上書きを許す)。
        if npm_packages
            .iter()
            .map(|(_, _, r, _)| r)
            .chain(crate_info.iter().map(|(_, _, r, _)| r))
            .any(tauri_sync_protected)
        {
            return;
        }

        // 最初の npm パッケージの現在バージョンを参照用に取得
        let npm_current = npm_packages.first().map(|(_, _, _, v)| v.as_str());

        // 保留中の更新後の実効バージョンが既にメジャー.マイナーで一致して
        // いれば同期不要。
        //
        // 判定は npm パッケージ全件で行う。`@tauri-apps/api` と `@tauri-apps/cli` は
        // パッチ集合差や age フィルタで judge 結果が非対称になりうるため、先頭 1 件
        // (依存はキー順に並ぶので通常 `api`) の一致だけで打ち切ると、残りのパッケージの
        // ずれが同期されないまま放置される。
        let crate_effective = crate_info
            .as_ref()
            .map(|(_, _, r, current)| effective_version(r, current));
        let all_npm_aligned = !npm_packages.is_empty()
            && npm_packages.iter().all(|(_, _, r, current)| {
                same_effective_major_minor(Some(effective_version(r, current)), crate_effective)
            });
        if all_npm_aligned {
            return;
        }

        // バージョンが不一致 - 同期が必要。両レジストリから候補を取得する
        let Some((npm_versions, crate_versions, cutoff)) =
            self.fetch_tauri_sync_versions(errors, project_dir).await
        else {
            return;
        };

        // 同期ヘルパーを作成し、同期後のバージョンを取得
        let sync = TauriVersionSync::new(npm_versions, crate_versions.clone());

        let npm_update_result = npm_packages.first().map(|(_, _, r, _)| r);
        let crate_update_result = crate_info.as_ref().map(|(_, _, r, _)| r);

        let (npm_target_version, crate_target_version) = sync.synchronize_with_current(
            npm_current,
            npm_update_result,
            crate_info.as_ref().map(|(_, _, _, v)| v.as_str()),
            crate_update_result,
        );

        // npm 側の調整を全 Tauri npm パッケージへ適用
        if let Some(ref target) = npm_target_version {
            self.apply_npm_sync_adjustments(summary, &npm_packages, target, cutoff)
                .await;
        }

        // crateバージョンの調整を適用
        if let Some(ref target) = crate_target_version
            && let Some((manifest_idx, result_idx, original, _)) = crate_info.as_ref()
        {
            apply_sync_adjustment(
                summary,
                *manifest_idx,
                *result_idx,
                original,
                target.clone(),
                self.resolved_age_policy_for(&project_dir.join("src-tauri"))
                    .cutoff()
                    .and_then(|cutoff| {
                        crate_versions
                            .iter()
                            .find(|info| &info.version == target)
                            .filter(|info| info.released_at > cutoff)
                    })
                    .and_then(|info| {
                        self.resolved_age_policy_for(&project_dir.join("src-tauri"))
                            .exemptions
                            .exemption(Language::Rust, TAURI_CRATE, info)
                    }),
            );
        }
    }

    /// Tauri 同期用に npm / crates.io 両レジストリからバージョン一覧を取得し、
    /// judge と同じ解決済み age の cutoff を適用して返す。
    ///
    /// 取得失敗時は `errors` へ積んで `None` を返す (呼び出し側は同期を中止する)。
    async fn fetch_tauri_sync_versions(
        &self,
        errors: &mut Vec<OrchestratorError>,
        project_dir: &Path,
    ) -> Option<(
        Vec<VersionInfo>,
        Vec<VersionInfo>,
        Option<chrono::DateTime<chrono::Utc>>,
    )> {
        let npm_adapter = self.get_adapter(Language::Node);
        let crate_adapter = self.get_adapter(Language::Rust);

        // バージョン取得には最初のnpmパッケージ名を使用 (全パッケージでバージョンは共通)
        let npm_versions = match self
            .fetch_for_tauri_sync(&*npm_adapter, TAURI_NPM_PACKAGES[0])
            .await
        {
            Ok(v) => v,
            Err(e) => {
                errors.push(e);
                return None;
            }
        };

        let crate_versions = match self
            .fetch_for_tauri_sync(&*crate_adapter, TAURI_CRATE)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                errors.push(e);
                return None;
            }
        };

        // 同期先の候補にも judge と同じ解決済み age を適用する
        // (同期が age ポリシーを迂回して新しすぎるバージョンを書かないように)
        let npm_policy = self.resolved_age_policy_for(project_dir);
        let crate_policy = self.resolved_age_policy_for(&project_dir.join("src-tauri"));
        let cutoff = npm_policy.cutoff();
        let crate_cutoff = crate_policy.cutoff();
        Some((
            filter_versions_by_cutoff(
                npm_versions,
                cutoff,
                Language::Node,
                TAURI_NPM_PACKAGES[0],
                &npm_policy.exemptions,
            ),
            filter_versions_by_cutoff(
                crate_versions,
                crate_cutoff,
                Language::Rust,
                TAURI_CRATE,
                &crate_policy.exemptions,
            ),
            cutoff,
        ))
    }

    /// npm 側の同期調整を全 Tauri npm パッケージへ適用する。
    ///
    /// @tauri-apps/api と @tauri-apps/cli はパッチバージョン集合が一致しない
    /// ことがあるため、パッケージごとに実在するバージョンを選ぶ。
    async fn apply_npm_sync_adjustments(
        &self,
        summary: &mut UpdateSummary,
        npm_packages: &[(usize, usize, UpdateResult, String)],
        target: &str,
        cutoff: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let npm_adapter = self.get_adapter(Language::Node);
        let npm_pkg_name = TAURI_NPM_PACKAGES[0];
        for (manifest_idx, result_idx, original, _current) in npm_packages {
            let pkg_name = original.package_name();
            let pkg_target = if pkg_name == npm_pkg_name {
                Some(target.to_string())
            } else {
                match self.fetch_versions(&*npm_adapter, pkg_name).await {
                    Ok(vs) => {
                        let vs = filter_versions_by_cutoff(
                            vs,
                            cutoff,
                            Language::Node,
                            pkg_name,
                            &self
                                .resolved_age_policy_for(
                                    summary.manifests[*manifest_idx]
                                        .path
                                        .parent()
                                        .unwrap_or(&self.args.path),
                                )
                                .exemptions,
                        );
                        pick_sync_version(&vs, target)
                    }
                    // フェッチできない場合はこのパッケージの同期を見送る
                    Err(_) => None,
                }
            };
            let Some(pkg_target) = pkg_target else {
                continue;
            };
            apply_sync_adjustment(
                summary,
                *manifest_idx,
                *result_idx,
                original,
                pkg_target,
                None,
            );
        }
    }

    /// Tauri 同期用にレジストリからバージョンを取得し、失敗時は
    /// `RegistryError` に変換して返す (呼び出し側で早期 return する)。
    async fn fetch_for_tauri_sync(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> Result<Vec<VersionInfo>, OrchestratorError> {
        self.fetch_versions(adapter, package)
            .await
            .map_err(|e| OrchestratorError::RegistryError {
                package: package.to_string(),
                message: format!("Failed to fetch for Tauri sync: {}", e),
            })
    }
}

/// バージョンチェック並列処理で 1 件の依存から得られた結果
/// (内部利用のみ)。
struct OnePassResult {
    name: String,
    outcome: UpdateResult,
    /// fetch 失敗時の原因メッセージ (存在する場合のみ `OrchestratorError` として記録される)
    fetch_error: Option<String>,
    /// OSV チェックで除外・問題が生じた version のメッセージ
    osv_warnings: Vec<String>,
}

/// `enforce_lock_age_rust` の実行結果
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockAgeAuditResult {
    /// 期間を満たさなかった `(crate 名, 元の版)` ごとの最終結果 (名前 → 版の順)。
    /// 1 件ずつの差し戻しで失敗し、後のまとめ解きで戻せた組は、戻せた結果だけが残る
    pub adjustments: Vec<LockAgeAdjustment>,
    /// 監査を終えた時点の Cargo.lock に残る、install で変わった版のうち公開日を確かめられ
    /// なかった件数 (時間予算 `LOCK_AGE_AUDIT_BUDGET` 切れなど。0 なら全件を確かめた)
    pub unchecked: usize,
    /// 利用者に必ず知らせるべき問題 (まとめ解きの後に元の Cargo.lock を戻せなかった等)
    pub problems: Vec<String>,
}

/// install 前の Cargo.lock (`baseline`) から見て、新規に入った / バージョンが変わった
/// registry エントリだけを名前順で抽出する。
///
/// post-install age 監査の対象を「depup の更新によって lock に入ったもの」へ限定する
/// ための絞り込み。lock 全体を監査すると crates.io の 1 リクエスト/秒 制限により
/// 数百件 = 数分の無音待ちになるうえ、depup が触っていない既存の解決結果まで
/// 差し戻し対象にしてしまう。
///
/// `baseline` が空の場合 (install 前に Cargo.lock が無かった等) は全エントリが
/// 「新規に入ったもの」なので、結果的に全件が対象になる。
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

/// `enforce_lock_age_rust` が 1 件の依存に対して実施した調整内容
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockAgeAdjustment {
    /// 対象パッケージ名
    pub name: String,
    /// Cargo.lock 上で解決されていたバージョン
    pub from: String,
    /// 差し戻したバージョン (`Downgraded` のみ)
    pub to: Option<String>,
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
struct LockAgeAudit<'a> {
    /// Cargo.lock のあるディレクトリ (workspace root)
    project_dir: &'a Path,
    /// これより後に公開された版を違反とする
    cutoff: chrono::DateTime<chrono::Utc>,
    exemptions: &'a crate::update::AgeExemptions,
    /// install 前の Cargo.lock (これと同じ版は監査しない)
    baseline: &'a RegistryLockEntries,
    /// judge がこの実行で選んだ版
    preferred: &'a PreferredVersions,
    /// 公開日の取得元 (本番は crates.io)
    adapter: &'a (dyn RegistryAdapter + Send + Sync),
    /// cargo の起動設定
    cargo: &'a CargoCommand,
    /// 監査全体の時間予算
    budget: Duration,
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
        self.entries.insert(
            (name.to_string(), from.to_string()),
            LockAgeAdjustment {
                name: name.to_string(),
                from: from.to_string(),
                to,
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
        self.entries
            .entry((name.to_string(), from.to_string()))
            .or_insert_with(|| LockAgeAdjustment {
                name: name.to_string(),
                from: from.to_string(),
                to,
                status,
            });
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
            if !adjustment.status.is_resolved() {
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
/// 該当が無ければ、期間を満たす版のうち今の版より古い最新 ([`pick_older_within_age`])。
///
/// どちらの場合も、install 前の lock (`before`) にあった同じ系列の版 ([`version_floor`])
/// より古くはしない。manifest の版要求が lock より古いまま (`^1.40.0` で lock は 1.50.0)
/// judge が `--max-change` で 1.40.x を選んだ場合や、install 前から期間内の版が入っていた
/// 場合に、install 前より古い版まで下げると、それまで使えていた API が消えてビルドが壊れる。
/// 下限より古い版しか無ければ install 前の版へ戻す (depup が入れた変更だけを取り消す)
fn rollback_target(
    available: &[VersionInfo],
    current: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
    preferred: Option<&[String]>,
    before: Option<&[String]>,
    exemptions: &crate::update::AgeExemptions,
) -> Option<String> {
    let floor = version_floor(before, current);
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
        Some(version) if at_least_floor(&version) => Some(version),
        // 期間を満たす版が下限より古い (または無い) なら、install 前の版に戻す
        _ => floor,
    }
}

/// install 前の lock (`before`) にあった、`current` と同じ semver 系列で `current` より古い版の
/// うち最新。差し戻しでこれより古くしない下限
fn version_floor(before: Option<&[String]>, current: &str) -> Option<String> {
    before
        .unwrap_or_default()
        .iter()
        .filter(|version| same_series(version, current))
        .filter(|version| compare_versions(version, current) == std::cmp::Ordering::Less)
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

/// `judge` の判定結果に対し、採用しようとした候補だけ OSV.dev に問い合わせ、
/// 脆弱性が見つかればその候補を除外して再 judge するループ。
///
/// 動作:
/// 1. 全 versions で judge → `UpdateResult::Update { new_version }` を取得
/// 2. その `new_version` を OSV に問い合わせ
///    - Safe → そのまま採用
///    - Vulnerable → versions から該当を除き、警告を残してループ再開
///    - API エラー → 元の候補を採用し、警告を残して終了 (チェック不能は安全側)
/// 3. `UpdateResult::Skip` (= 更新不要) はそのまま返す
///
/// 通常 1 依存あたり 1〜2 API call で済む。
/// 全 candidate を網羅的にチェックする旧実装と違い、`@angular/*` のように
/// 1000+ バージョンを持つパッケージでも実用的な速度で完了する。
async fn judge_with_osv(
    judge: &UpdateJudge,
    dep: &Dependency,
    versions: Vec<VersionInfo>,
    checker: &OsvChecker,
    ecosystem: &str,
    bar: Option<&ProgressBar>,
    warnings: &mut Vec<String>,
) -> UpdateResult {
    let mut allowed = versions;
    let mut fallback_chain: Vec<String> = Vec::new();
    loop {
        let result = judge.judge(dep, &allowed);
        let UpdateResult::Update {
            new_version: target,
            ..
        } = &result
        else {
            // Skip 結果は OSV と無関係に確定
            return result;
        };
        let target = target.clone();

        if let Some(b) = bar {
            b.set_message(format!("OSV: {} {}", dep.name, target));
        }

        match checker.check(ecosystem, &dep.name, &target).await {
            Ok(OsvCheck::Safe) => {
                if !fallback_chain.is_empty() {
                    let line = format!(
                        "  ↓ {}: skipped {} due to OSV → using {}",
                        dep.name,
                        fallback_chain.join(", "),
                        target
                    );
                    osv_println(bar, &line);
                    return result
                        .with_osv_skipped(fallback_chain)
                        .with_osv_checked(true);
                }
                // チェック完了・脆弱性なし
                return result.with_osv_checked(true);
            }
            Ok(OsvCheck::Vulnerable(ids)) => {
                let detail = if ids.is_empty() {
                    "no advisory IDs".to_string()
                } else {
                    ids.join(", ")
                };
                let line = format!("  ⚠ OSV: {} {} vulnerable ({})", dep.name, target, detail);
                osv_println(bar, &line);

                fallback_chain.push(format!("{} ({})", target, detail));
                warnings.push(format!("{} vulnerable, falling back ({})", target, detail));
                let before = allowed.len();
                // Python の PEP 440 ローカルバージョン (`1.0+cu121` 等) は build metadata を
                // 無視する semver 比較では区別できず、安全な候補まで除外して NoSuitableVersion に
                // 落としてしまう。compare_dependency_versions で言語別比較に切り替える。
                allowed.retain(|v| {
                    compare_dependency_versions(dep, &v.version, &target)
                        != std::cmp::Ordering::Equal
                });
                if allowed.len() == before {
                    // 除外できなかった (compare_versions の都合) → 無限ループ防止
                    let line = format!(
                        "  ⚠ {}: could not exclude {} from candidates, keeping it",
                        dep.name, target
                    );
                    osv_println(bar, &line);
                    warnings.push(format!(
                        "could not exclude {} from candidates, stopping OSV check",
                        target
                    ));
                    return result.with_osv_skipped(fallback_chain);
                }
                // ループ継続 → 次の候補で再判定
            }
            Err(e) => {
                let line = format!("  ⚠ OSV check failed for {} {}: {}", dep.name, target, e);
                osv_println(bar, &line);
                warnings.push(format!("OSV check failed for {}: {}", target, e));
                return if fallback_chain.is_empty() {
                    result
                } else {
                    result.with_osv_skipped(fallback_chain)
                };
            }
        }
    }
}

/// 進捗バーがあれば `println` で行を出力 (バーを維持)、なければ stderr へ直接出す。
fn osv_println(bar: Option<&ProgressBar>, line: &str) {
    match bar {
        Some(b) => b.println(line),
        None => eprintln!("{}", line),
    }
}

/// オーケストレータの設定
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// 汎用レジストリの最大同時リクエスト数
    pub general_concurrency: usize,
    /// crates.io の最大同時リクエスト数
    pub crates_io_concurrency: usize,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            general_concurrency: DEFAULT_CONCURRENCY,
            crates_io_concurrency: CRATES_IO_CONCURRENCY,
        }
    }
}

/// 現在の lock バージョンより古く、かつ cutoff 以前にリリースされた
/// 候補の中から semver 最新のものを選ぶ。
/// プレリリースは除外する。
fn pick_older_within_age(
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
        Err(error) => LockAgeStatus::UpdateCommandFailed(error.to_string()),
    }
}

/// Cargo.toml に対応する Cargo.lock を読み込み、git 依存の `current_commit` をセットする。
///
/// workspace メンバーの lock はワークスペースルートにのみ存在するため、
/// マニフェストのディレクトリから `boundary` まで上方向に探す。
fn enrich_with_cargo_lock(
    cargo_toml_path: &Path,
    boundary: &Path,
    dependencies: &mut [Dependency],
) {
    let Some(dir) = cargo_toml_path.parent() else {
        return;
    };
    let Some(lock_path) = find_cargo_lock_upward(dir, boundary) else {
        return;
    };
    let git_entries = read_git_entries(&lock_path);
    if git_entries.is_empty() {
        return;
    }
    for dep in dependencies.iter_mut() {
        if let Some(git) = dep.git_source.as_mut()
            && let Some(entries) = git_entries.get(&dep.name)
        {
            // 同名エントリが複数ある場合 (fork と upstream の併用等) は URL で
            // 対応付ける。一致が無く候補が 1 件だけなら従来どおりそれを使う。
            let url_matched = entries.iter().find(|e| git_urls_match(&e.url, &git.url));
            let matched = match (url_matched, entries.as_slice()) {
                (Some(entry), _) => Some(entry),
                (None, [only]) => Some(only),
                (None, _) => None,
            };
            if let Some(entry) = matched {
                git.current_commit = Some(entry.commit.clone());
            }
        }
    }
}

/// git URL 同士を末尾の `/` と `.git` の差を無視して比較する
fn git_urls_match(a: &str, b: &str) -> bool {
    fn normalize(url: &str) -> &str {
        let url = url.trim_end_matches('/');
        url.strip_suffix(".git").unwrap_or(url)
    }
    normalize(a) == normalize(b)
}

/// Tauri バージョン同期が judge の結果を上書きしてはならないかどうか。
///
/// ユーザの明示的なフィルタ (--exclude / --only / 言語フィルタ / pinned /
/// --max-change) や判定不能 (fetch・parse 失敗) によるスキップを同期が
/// 上書きすると、指定を破ってマニフェストへ書き込んでしまう。
///
/// OSV フォールバックで脆弱な候補を退けた結果も保護する。同期先の選定は
/// レジストリの生のバージョン一覧から最新を選ぶため、保護しないと judge が
/// 除外したはずの脆弱版を選び直して書き戻し、OSV チェックを無効化したのと
/// 同じ結果になる。
fn tauri_sync_protected(result: &UpdateResult) -> bool {
    match result {
        UpdateResult::Update { osv_skipped, .. } => !osv_skipped.is_empty(),
        UpdateResult::Skip { reason, .. } => !matches!(
            reason,
            SkipReason::AlreadyLatest | SkipReason::NoSuitableVersion
        ),
    }
}

/// 保留中の更新を織り込んだ実効バージョンを返す。
/// Update なら更新後のバージョン、それ以外 (AlreadyLatest 等の Skip) は現在版。
fn effective_version<'a>(result: &'a UpdateResult, current: &'a str) -> &'a str {
    match result {
        UpdateResult::Update { new_version, .. } => new_version.as_str(),
        _ => current,
    }
}

/// npm 側と crate 側の実効バージョンが同じメジャー.マイナーに揃っているか判定する。
/// どちらかが欠けている・メジャー.マイナーを抽出できない場合は false
/// (= 同期処理を継続する)。
fn same_effective_major_minor(npm_version: Option<&str>, crate_version: Option<&str>) -> bool {
    use crate::tauri_sync::extract_major_minor;

    if let (Some(npm_v), Some(crate_v)) = (npm_version, crate_version)
        && let (Some(npm_mm), Some(crate_mm)) =
            (extract_major_minor(npm_v), extract_major_minor(crate_v))
    {
        npm_mm == crate_mm
    } else {
        false
    }
}

/// summary 内の指定言語マニフェストから、`is_match` に一致するパッケージの
/// (manifest_idx, result_idx, 結果のクローン, 現在バージョン) を集める。
fn collect_tauri_packages(
    summary: &UpdateSummary,
    project_dir: &Path,
    language: Language,
    is_match: impl Fn(&str) -> bool,
) -> Vec<(usize, usize, UpdateResult, String)> {
    summary
        .manifests
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.language == language
                && match language {
                    Language::Node => m.path == project_dir.join("package.json"),
                    Language::Rust => m.path == project_dir.join("src-tauri/Cargo.toml"),
                    _ => false,
                }
        })
        .flat_map(|(mi, m)| {
            m.results
                .iter()
                .enumerate()
                .filter(|(_, r)| is_match(r.package_name()))
                .map(move |(ri, r)| {
                    let current = r.dependency().version().to_string();
                    (mi, ri, r.clone(), current)
                })
        })
        .collect()
}

/// Tauri 同期で決定したターゲットバージョンを summary の該当結果へ反映する。
/// 元が Skip だった場合はマニフェストを modified としてマークする。
fn apply_sync_adjustment(
    summary: &mut UpdateSummary,
    manifest_idx: usize,
    result_idx: usize,
    original: &UpdateResult,
    target: String,
    exemption: Option<crate::update::AgeExemption>,
) {
    // 同期先が現在版と同一なら Update を作らない。
    // judge 側は「書き換え結果が現在の raw と同一なら AlreadyLatest」という不変条件を
    // 持つが (phantom update 防止)、Tauri 同期はそれを迂回するため同じ判定をここで行う。
    // これがないと npm 側が crates.io より先行しているとき `2.9.6 → 2.9.6` が毎回
    // 「更新」として報告され、writer は何も書かないのに `--install` だけが走る。
    if original.dependency().version() == target {
        return;
    }

    let adjusted =
        UpdateResult::update(original.dependency().clone(), target).with_age_exemption(exemption);
    summary.manifests[manifest_idx].results[result_idx] = adjusted;
    if matches!(original, UpdateResult::Skip { .. }) {
        summary.manifests[manifest_idx].modified = true;
    }
}

/// cutoff (現在時刻 - age) より新しいバージョンを候補から除外する
fn filter_versions_by_cutoff(
    versions: Vec<VersionInfo>,
    cutoff: Option<chrono::DateTime<chrono::Utc>>,
    language: Language,
    package: &str,
    exemptions: &crate::update::AgeExemptions,
) -> Vec<VersionInfo> {
    match cutoff {
        Some(c) => versions
            .into_iter()
            .filter(|v| exemptions.admits(language, package, v, c))
            .collect(),
        None => versions,
    }
}

/// 同期先バージョンをパッケージ自身のバージョン一覧から選ぶ。
///
/// target がそのまま存在すればそれを、無ければ同じ major.minor 系列の
/// 最新安定版を選ぶ (`@tauri-apps/api` と `@tauri-apps/cli` でパッチ
/// バージョン集合が異なるケースに対応)。
fn pick_sync_version(versions: &[VersionInfo], target: &str) -> Option<String> {
    use crate::tauri_sync::extract_major_minor;

    if versions.iter().any(|v| v.version == target) {
        return Some(target.to_string());
    }
    let target_mm = extract_major_minor(target)?;
    versions
        .iter()
        .filter(|v| !crate::update::is_prerelease_version(&v.version))
        .filter(|v| extract_major_minor(&v.version).is_some_and(|mm| mm == target_mm))
        .max_by(|a, b| compare_versions(&a.version, &b.version))
        .map(|v| v.version.clone())
}

/// リモート commit と現在 commit を比較し、差分があれば更新結果を作る
fn compare_and_update_commit(
    dep: &Dependency,
    latest_commit: &str,
    current_commit: Option<&str>,
) -> UpdateResult {
    match current_commit {
        Some(current) if current == latest_commit => UpdateResult::skip_already_latest(dep.clone()),
        _ => UpdateResult::update(dep.clone(), latest_commit.to_string()),
    }
}

/// タグが semver 形状 (`v1.2.3` / `1.2` / `1.2.3-rc.1+build`) かどうか。
///
/// 日付タグ (`2024.06.01` は形状上区別できないが `20240601-hotfix` 等) や
/// CI 用タグを「最新 semver タグ」候補から除外するための形状チェック。
/// 数値コアは 2〜3 セグメントのみ許容する (1 セグメントは日付 `20240601` と
/// 区別できないため除外)。
fn looks_like_semver_tag(tag: &str) -> bool {
    let body = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
    let core_end = body.find(['-', '+']).unwrap_or(body.len());
    let core = &body[..core_end];
    let segments: Vec<&str> = core.split('.').collect();
    if !(2..=3).contains(&segments.len()) {
        return false;
    }
    segments
        .iter()
        .all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
}

/// 指定されたタグ群から最新の semver 互換タグを選ぶ。
/// プレリリースと semver 形状でないタグ (日付タグ等) は除外する。
fn latest_semver_tag(tags: &[String]) -> Option<String> {
    let stable: Vec<&String> = tags
        .iter()
        .filter(|t| !crate::update::is_prerelease_version(t))
        .filter(|t| looks_like_semver_tag(t))
        .collect();
    if stable.is_empty() {
        return None;
    }
    let mut latest: &String = stable[0];
    for tag in stable.iter().skip(1) {
        if compare_versions(tag, latest) == std::cmp::Ordering::Greater {
            latest = tag;
        }
    }
    Some(latest.clone())
}

#[cfg(test)]
mod git_helper_tests {
    use super::*;
    use crate::domain::{GitReference, GitSource, VersionSpec, VersionSpecKind};

    fn git_dep(name: &str, reference: GitReference) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Exact, "main", "main");
        let dep = Dependency::new(name, spec, false, Language::Rust);
        dep.with_git_source(GitSource::new("https://example.com/r.git", reference))
    }

    #[test]
    fn test_latest_semver_tag_basic() {
        let tags = vec![
            "v0.1.0".to_string(),
            "v1.2.3".to_string(),
            "v1.2.4".to_string(),
        ];
        assert_eq!(latest_semver_tag(&tags), Some("v1.2.4".to_string()));
    }

    #[test]
    fn test_latest_semver_tag_filters_prereleases() {
        let tags = vec![
            "v1.0.0".to_string(),
            "v1.1.0-beta.1".to_string(),
            "v1.0.5".to_string(),
        ];
        // プレリリースは除外、v1.0.5 が最新
        assert_eq!(latest_semver_tag(&tags), Some("v1.0.5".to_string()));
    }

    #[test]
    fn test_latest_semver_tag_empty() {
        assert_eq!(latest_semver_tag(&[]), None);
    }

    #[test]
    fn test_latest_semver_tag_all_prereleases() {
        let tags = vec!["v1.0.0-alpha".to_string(), "v1.0.0-beta".to_string()];
        assert_eq!(latest_semver_tag(&tags), None);
    }

    #[test]
    fn test_latest_semver_tag_ignores_date_and_ci_tags() {
        // 日付タグや CI 用タグが semver タグより「大きい」数値でも選ばれない
        let tags = vec![
            "v1.2.3".to_string(),
            "20240601-hotfix".to_string(),
            "20250101".to_string(),
            "release-2025".to_string(),
            "nightly".to_string(),
        ];
        assert_eq!(latest_semver_tag(&tags), Some("v1.2.3".to_string()));
    }

    #[test]
    fn test_looks_like_semver_tag() {
        assert!(looks_like_semver_tag("v1.2.3"));
        assert!(looks_like_semver_tag("1.2"));
        assert!(looks_like_semver_tag("V2.0.0"));
        assert!(looks_like_semver_tag("1.2.3-rc.1+build"));
        assert!(!looks_like_semver_tag("20240601"));
        assert!(!looks_like_semver_tag("20240601-hotfix"));
        assert!(!looks_like_semver_tag("1.2.3.4"));
        assert!(!looks_like_semver_tag("nightly"));
        assert!(!looks_like_semver_tag("v1"));
    }

    #[test]
    fn test_git_urls_match_ignores_git_suffix_and_slash() {
        assert!(git_urls_match(
            "https://github.com/a/b.git",
            "https://github.com/a/b"
        ));
        assert!(git_urls_match(
            "https://github.com/a/b/",
            "https://github.com/a/b"
        ));
        assert!(!git_urls_match(
            "https://github.com/a/b",
            "https://github.com/a/c"
        ));
    }

    #[test]
    fn test_tauri_sync_protected_reasons() {
        let dep = git_dep("tauri", GitReference::DefaultBranch);

        // 明示的フィルタ・判定不能系は保護される
        for reason in [
            SkipReason::Excluded,
            SkipReason::NotInOnlyList,
            SkipReason::LanguageFiltered,
            SkipReason::Pinned,
            SkipReason::ChangeLevelLimited(crate::domain::ChangeLevel::Patch),
            SkipReason::FetchFailed("boom".to_string()),
            SkipReason::ParseError("bad".to_string()),
        ] {
            assert!(
                tauri_sync_protected(&UpdateResult::skip(dep.clone(), reason.clone())),
                "{:?} は同期で上書きしないべき",
                reason
            );
        }

        // 同期による上書きを許すケース
        assert!(!tauri_sync_protected(&UpdateResult::skip(
            dep.clone(),
            SkipReason::AlreadyLatest
        )));
        assert!(!tauri_sync_protected(&UpdateResult::skip(
            dep.clone(),
            SkipReason::NoSuitableVersion
        )));
        assert!(!tauri_sync_protected(&UpdateResult::update(dep, "2.0.0")));
    }

    #[test]
    fn test_effective_version_pending_update_takes_precedence() {
        let dep = git_dep("tauri", GitReference::DefaultBranch);

        // Update は更新後バージョンを実効値とする
        assert_eq!(
            effective_version(&UpdateResult::update(dep.clone(), "2.1.0"), "2.0.0"),
            "2.1.0"
        );
        // Skip (AlreadyLatest 等) は現在版を実効値とする
        assert_eq!(
            effective_version(
                &UpdateResult::skip(dep.clone(), SkipReason::AlreadyLatest),
                "2.0.0"
            ),
            "2.0.0"
        );
        assert_eq!(
            effective_version(&UpdateResult::skip(dep, SkipReason::Pinned), "1.5.3"),
            "1.5.3"
        );
    }

    #[test]
    fn test_same_effective_major_minor_alignment() {
        // 同じ major.minor なら整合 (パッチ差は同期不要)
        assert!(same_effective_major_minor(Some("2.0.0"), Some("2.0.5")));
        // minor 違いは不整合
        assert!(!same_effective_major_minor(Some("2.0.0"), Some("2.1.0")));
        // major 違いは不整合
        assert!(!same_effective_major_minor(Some("1.9.0"), Some("2.9.0")));
        // 片側欠落・両側欠落は「整合とは判定しない」(同期継続側に倒す)
        assert!(!same_effective_major_minor(Some("2.0.0"), None));
        assert!(!same_effective_major_minor(None, Some("2.0.0")));
        assert!(!same_effective_major_minor(None, None));
        // メジャー.マイナーを抽出できない文字列も整合扱いしない
        assert!(!same_effective_major_minor(Some("latest"), Some("2.0.0")));
    }

    #[test]
    fn test_pick_sync_version_prefers_exact_then_same_minor() {
        use crate::update::VersionInfo;
        use chrono::Utc;

        let versions = vec![
            VersionInfo::new("2.9.0", Utc::now()),
            VersionInfo::new("2.9.6", Utc::now()),
            VersionInfo::new("2.10.0-beta.1", Utc::now()),
        ];

        // 完全一致があればそれを使う
        assert_eq!(
            pick_sync_version(&versions, "2.9.0"),
            Some("2.9.0".to_string())
        );
        // 無ければ同じ major.minor の最新安定版 (プレリリース除外)
        assert_eq!(
            pick_sync_version(&versions, "2.9.3"),
            Some("2.9.6".to_string())
        );
        // 系列ごと存在しなければ None (同期を見送る)
        assert_eq!(pick_sync_version(&versions, "3.0.0"), None);
    }

    #[test]
    fn test_compare_and_update_commit_new() {
        let dep = git_dep("foo", GitReference::Branch("main".to_string()));
        let result = compare_and_update_commit(&dep, "new_sha_0000000000000000", Some("old_sha"));
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "new_sha_0000000000000000");
        }
    }

    #[test]
    fn test_compare_and_update_commit_same() {
        let dep = git_dep("foo", GitReference::Branch("main".to_string()));
        let result = compare_and_update_commit(&dep, "abc", Some("abc"));
        assert!(result.is_skip());
    }

    #[test]
    fn test_compare_and_update_commit_no_current() {
        let dep = git_dep("foo", GitReference::DefaultBranch);
        // 現在 commit 不明でも更新として扱う (lock 未生成時に lock 生成を誘発)
        let result = compare_and_update_commit(&dep, "newsha", None);
        assert!(result.is_update());
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
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn monorepo_candidate_selection_respects_local_age() {
        let root = TempDir::new().unwrap();
        let app = root.path().join("app");
        fs::create_dir_all(&app).unwrap();
        for dir in [root.path(), app.as_path()] {
            fs::write(
                dir.join("package.json"),
                r#"{"dependencies":{"library":"^1.0.0"}}"#,
            )
            .unwrap();
        }
        fs::write(app.join(".npmrc"), "minimum-release-age=30d\n").unwrap();
        let orchestrator = Orchestrator::new(make_args_with_path(
            root.path(),
            &["--no-age", "--no-osv", "--dry-run", "--quiet"],
        ))
        .unwrap();
        let now = chrono::Utc::now();
        orchestrator.version_cache.lock().await.insert(
            (Language::Node, "library".into()),
            vec![
                VersionInfo::new("1.0.0", now - chrono::Duration::days(60)),
                VersionInfo::new("2.0.0", now - chrono::Duration::days(3)),
            ],
        );
        let result = orchestrator
            .run_directories(&[root.path().into(), app.clone()])
            .await;
        assert!(result.errors.is_empty());
        let root_result = result
            .summary
            .manifests
            .iter()
            .find(|manifest| manifest.path == root.path().join("package.json"))
            .unwrap();
        let app_result = result
            .summary
            .manifests
            .iter()
            .find(|manifest| manifest.path == app.join("package.json"))
            .unwrap();
        assert_eq!(root_result.updates().count(), 1);
        assert_eq!(app_result.updates().count(), 0);
    }

    #[tokio::test]
    async fn tauri_sync_is_scoped_to_each_project_age_policy() {
        let root = TempDir::new().unwrap();
        let app = root.path().join("app");
        let other = root.path().join("other");
        for dir in [&app, &other] {
            fs::create_dir_all(dir.join("src-tauri")).unwrap();
            fs::write(
                dir.join("package.json"),
                r#"{"dependencies":{"@tauri-apps/api":"^2.0.0","@tauri-apps/cli":"^2.0.0"}}"#,
            )
            .unwrap();
            fs::write(
                dir.join("src-tauri/Cargo.toml"),
                "[package]\nname = 'app'\nversion = '0.1.0'\n[dependencies]\ntauri = '2.0.0'\n",
            )
            .unwrap();
        }
        fs::write(other.join(".npmrc"), "minimum-release-age=30d\n").unwrap();
        let orchestrator = Orchestrator::new(make_args_with_path(
            root.path(),
            &["--no-age", "--no-osv", "--dry-run", "--quiet"],
        ))
        .unwrap();
        let now = chrono::Utc::now();
        let versions = vec![
            VersionInfo::new("2.0.1", now - chrono::Duration::days(60)),
            VersionInfo::new("2.2.0", now - chrono::Duration::days(3)),
        ];
        let mut npm_versions = versions.clone();
        npm_versions.push(VersionInfo::new("2.3.0", now - chrono::Duration::days(3)));
        {
            let mut cache = orchestrator.version_cache.lock().await;
            for name in TAURI_NPM_PACKAGES {
                cache.insert((Language::Node, (*name).into()), npm_versions.clone());
            }
            cache.insert((Language::Rust, TAURI_CRATE.into()), versions);
        }
        let result = orchestrator
            .run_directories(&[app.clone(), other.clone()])
            .await;
        assert!(result.errors.is_empty());
        for manifest in &result.summary.manifests {
            let expected = if manifest.path.starts_with(&other) {
                "2.0.1"
            } else {
                "2.2.0"
            };
            for update in manifest.updates() {
                assert!(
                    matches!(update, UpdateResult::Update { new_version, .. } if new_version == expected),
                    "{update:?}"
                );
            }
            assert!(manifest.has_updates());
        }
    }

    #[test]
    fn explicit_project_age_overrides_global_publisher_exemptions() {
        use crate::domain::{VersionSpec, VersionSpecKind};
        let dir = TempDir::new().unwrap();
        let config = GlobalConfig {
            age: Some("2w".into()),
            age_exempt: crate::update::AgeExemptions {
                github: vec!["example-dev".into()],
            },
            ..Default::default()
        };
        let orchestrator = Orchestrator::new(make_args_with_path(dir.path(), &["--rust"]))
            .unwrap()
            .with_global_config(Some(config));
        let dependency = Dependency::new(
            "library",
            VersionSpec::new(VersionSpecKind::Caret, "^1.0.0", "1.0.0"),
            false,
            Language::Rust,
        );
        let mut young = VersionInfo::now("1.1.0");
        young.publisher = crate::update::PublisherEvidence::GithubUser {
            login: "example-dev".into(),
        };
        let versions = [
            VersionInfo::new("1.0.0", chrono::Utc::now() - chrono::Duration::days(30)),
            young,
        ];
        assert!(
            UpdateJudge::new(orchestrator.build_filter())
                .judge(&dependency, &versions)
                .is_update()
        );
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();
        assert!(
            UpdateJudge::new(orchestrator.build_filter())
                .judge(&dependency, &versions)
                .is_skip()
        );
        assert!(orchestrator.resolved_age_exemptions().is_empty());
        assert_eq!(
            orchestrator.resolved_min_age(),
            Some(Duration::from_secs(10 * 86400))
        );
    }

    #[test]
    fn rollback_can_select_a_young_verified_release() {
        let version_at = |version: &str, days: i64| {
            VersionInfo::new(version, chrono::Utc::now() - chrono::Duration::days(days))
        };
        let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
        let exemptions = crate::update::AgeExemptions {
            github: vec!["example-dev".into()],
        };
        let mut young = version_at("1.4.9", 1);
        young.publisher = crate::update::PublisherEvidence::GithubUser {
            login: "example-dev".into(),
        };
        let versions = vec![version_at("1.4.5", 30), young, version_at("1.5.0", 1)];
        assert_eq!(
            pick_older_within_age(&versions, "1.5.0", cutoff, &exemptions).as_deref(),
            Some("1.4.9")
        );
        let preferred = ["1.4.9".into()];
        assert_eq!(
            rollback_target(
                &versions,
                "1.5.0",
                cutoff,
                Some(&preferred),
                None,
                &exemptions
            )
            .as_deref(),
            Some("1.4.9")
        );
        assert_eq!(
            pick_older_within_age(&versions, "1.5.0", cutoff, &Default::default()).as_deref(),
            Some("1.4.5")
        );
        assert_eq!(
            filter_versions_by_cutoff(
                versions.clone(),
                Some(cutoff),
                Language::Rust,
                "library",
                &exemptions
            )
            .len(),
            2
        );
        assert_eq!(
            filter_versions_by_cutoff(
                versions,
                Some(cutoff),
                Language::Node,
                "library",
                &exemptions
            )
            .len(),
            1
        );
    }

    use super::*;
    use async_trait::async_trait;
    use clap::Parser;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    struct CountingAdapter {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl RegistryAdapter for CountingAdapter {
        fn language(&self) -> Language {
            Language::Node
        }

        fn registry_name(&self) -> &'static str {
            "counting"
        }

        async fn fetch_versions(
            &self,
            _package: &str,
        ) -> Result<Vec<VersionInfo>, crate::error::RegistryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            Ok(vec![VersionInfo::now("1.0.0")])
        }
    }

    fn make_args(args: &[&str]) -> CliArgs {
        CliArgs::parse_from(args)
    }

    fn make_args_with_path(path: &std::path::Path, extra_args: &[&str]) -> CliArgs {
        let path_str = path.to_str().unwrap();
        let mut args = vec!["depup", path_str];
        args.extend(extra_args);
        CliArgs::parse_from(&args)
    }

    #[test]
    fn test_orchestrator_config_default() {
        let config = OrchestratorConfig::default();
        assert_eq!(config.general_concurrency, 10);
        assert_eq!(config.crates_io_concurrency, 1);
    }

    #[test]
    fn test_version_check_concurrency_scaling() {
        // 依存数に応じて並列度が 1〜4 の範囲で伸縮する
        assert_eq!(version_check_concurrency(0), 1); // 0 件でも最小 1
        assert_eq!(version_check_concurrency(1), 1);
        assert_eq!(version_check_concurrency(2), 2);
        assert_eq!(version_check_concurrency(3), 3);
        assert_eq!(version_check_concurrency(4), 4);
        assert_eq!(version_check_concurrency(10), 4); // 上限に張り付く
        assert_eq!(version_check_concurrency(100), 4);
        assert_eq!(version_check_concurrency(usize::MAX), 4);
    }

    #[test]
    fn test_build_filter_no_args() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        // 言語フィルタなし
        assert!(filter.should_process_language(Language::Node));
        assert!(filter.should_process_language(Language::Python));
        assert!(filter.should_process_language(Language::Rust));
        assert!(filter.should_process_language(Language::Go));
    }

    #[test]
    fn test_build_filter_with_languages() {
        let args = make_args(&["depup", "--node", "--python"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(filter.should_process_language(Language::Node));
        assert!(filter.should_process_language(Language::Python));
        assert!(!filter.should_process_language(Language::Rust));
        assert!(!filter.should_process_language(Language::Go));
    }

    #[test]
    fn test_build_filter_with_exclude() {
        let args = make_args(&["depup", "--exclude", "lodash", "--exclude", "react"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(!filter.should_process_package("lodash"));
        assert!(!filter.should_process_package("react"));
        assert!(filter.should_process_package("express"));
    }

    #[test]
    fn test_build_filter_with_only() {
        let args = make_args(&["depup", "--only", "lodash"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(filter.should_process_package("lodash"));
        assert!(!filter.should_process_package("react"));
    }

    #[test]
    fn test_build_filter_with_include_pinned() {
        let args = make_args(&["depup", "--include-pinned"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(filter.include_pinned);
    }

    #[test]
    fn test_build_filter_with_age() {
        let args = make_args(&["depup", "--age", "2w"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(filter.min_age.is_some());
        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(14 * 24 * 60 * 60)
        );
    }

    #[test]
    fn test_should_process_language_no_filter() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();

        assert!(orchestrator.should_process_language(Language::Node));
        assert!(orchestrator.should_process_language(Language::Python));
        assert!(orchestrator.should_process_language(Language::Rust));
        assert!(orchestrator.should_process_language(Language::Go));
        assert!(orchestrator.should_process_language(Language::Java));
    }

    #[test]
    fn test_should_process_language_with_filter() {
        let args = make_args(&["depup", "--node"]);
        let orchestrator = Orchestrator::new(args).unwrap();

        assert!(orchestrator.should_process_language(Language::Node));
        assert!(!orchestrator.should_process_language(Language::Python));
        assert!(!orchestrator.should_process_language(Language::Rust));
        assert!(!orchestrator.should_process_language(Language::Go));
        assert!(!orchestrator.should_process_language(Language::Java));

        // Javaのみのフィルタをテスト
        let args = make_args(&["depup", "--java"]);
        let orchestrator = Orchestrator::new(args).unwrap();

        assert!(orchestrator.should_process_language(Language::Java));
        assert!(!orchestrator.should_process_language(Language::Node));
        assert!(!orchestrator.should_process_language(Language::Python));
    }

    #[test]
    fn test_get_adapter_node() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let adapter = orchestrator.get_adapter(Language::Node);
        assert_eq!(adapter.language(), Language::Node);
    }

    #[test]
    fn test_get_adapter_python() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let adapter = orchestrator.get_adapter(Language::Python);
        assert_eq!(adapter.language(), Language::Python);
    }

    #[test]
    fn test_get_adapter_rust() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let adapter = orchestrator.get_adapter(Language::Rust);
        assert_eq!(adapter.language(), Language::Rust);
    }

    #[test]
    fn test_get_adapter_go() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let adapter = orchestrator.get_adapter(Language::Go);
        assert_eq!(adapter.language(), Language::Go);
    }

    #[test]
    fn test_get_adapter_java() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let adapter = orchestrator.get_adapter(Language::Java);
        assert_eq!(adapter.language(), Language::Java);
    }

    #[test]
    fn test_orchestrator_error_display() {
        let err = OrchestratorError::HttpClientError("connection failed".to_string());
        assert!(err.to_string().contains("HTTP client error"));

        let err = OrchestratorError::ManifestDetectionError("not found".to_string());
        assert!(err.to_string().contains("Manifest detection error"));

        let err = OrchestratorError::ManifestParseError {
            path: "/path/to/file".to_string(),
            message: "invalid".to_string(),
        };
        assert!(err.to_string().contains("Failed to parse"));

        let err = OrchestratorError::RegistryError {
            package: "lodash".to_string(),
            message: "not found".to_string(),
        };
        assert!(err.to_string().contains("Failed to fetch lodash"));

        let err = OrchestratorError::WriteError {
            path: "/path/to/file".to_string(),
            message: "permission denied".to_string(),
        };
        assert!(err.to_string().contains("Failed to write"));
    }

    #[test]
    fn test_build_filter_with_pnpm_workspace_yaml() {
        let dir = TempDir::new().unwrap();

        // minimumReleaseAge を分単位で指定した pnpm-workspace.yaml を作成 (14400 = 10日)
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();

        let args = make_args_with_path(dir.path(), &[]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        // pnpm設定からmin_ageが設定されるべき (14400分 = 864000秒)
        assert!(filter.min_age.is_some());
        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(14400 * 60)
        );
    }

    #[test]
    fn test_build_filter_pnpm_overrides_cli() {
        let dir = TempDir::new().unwrap();

        // minimumReleaseAge 付きの pnpm-workspace.yaml を作成 (10日 = 14400分)
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();

        // CLI --age 2w を指定しても、プロジェクトポリシー (pnpm 10日) が勝つ
        let args = make_args_with_path(dir.path(), &["--age", "2w"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(filter.min_age.is_some());
        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(10 * 24 * 60 * 60), // pnpm の 10日
            "minimumReleaseAge は CLI --age に優先する"
        );
    }

    #[test]
    fn test_build_filter_bun_minimum_release_age() {
        let dir = TempDir::new().unwrap();
        // bunfig.toml に minimumReleaseAge (秒) を書く: 3日 = 259200 秒
        fs::write(
            dir.path().join("bunfig.toml"),
            "[install]\nminimumReleaseAge = 259200\n",
        )
        .unwrap();

        let args = make_args_with_path(dir.path(), &[]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(3 * 24 * 60 * 60),
        );
    }

    #[test]
    fn test_build_filter_pnpm_and_bun_take_max() {
        let dir = TempDir::new().unwrap();
        // pnpm: 10日, bun: 3日 → max=10日
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("bunfig.toml"),
            "[install]\nminimumReleaseAge = 259200\n",
        )
        .unwrap();

        let args = make_args_with_path(dir.path(), &[]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(10 * 24 * 60 * 60),
            "両方ある場合はより厳しい (max) を採用"
        );
    }

    #[test]
    fn test_build_filter_with_npmrc() {
        let dir = TempDir::new().unwrap();

        // pnpmプロジェクトであることを示す pnpm-lock.yaml を作成
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();

        // minimum-release-age 付きの .npmrc を作成
        fs::write(dir.path().join(".npmrc"), "minimum-release-age=10d\n").unwrap();

        let args = make_args_with_path(dir.path(), &[]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        // .npmrc からmin_ageが設定されるべき (10日)
        assert!(filter.min_age.is_some());
        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(10 * 24 * 60 * 60)
        );
    }

    #[test]
    fn test_build_filter_no_pnpm_no_age_falls_back_to_default() {
        let dir = TempDir::new().unwrap();

        // pnpm/bun 設定なし、CLI --age なし、global_config なし → 組み込みデフォルト (1w)
        let args = make_args_with_path(dir.path(), &[]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert_eq!(
            filter.min_age.unwrap(),
            crate::global_config::DEFAULT_AGE,
            "未指定時は組み込みデフォルト (1w) にフォールバック"
        );
    }

    #[test]
    fn test_build_filter_no_age_explicit_disables_when_no_project_settings() {
        let dir = TempDir::new().unwrap();

        // --no-age 指定、プロジェクト設定なし → age 制約なし
        let args = make_args_with_path(dir.path(), &["--no-age"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(
            filter.min_age.is_none(),
            "--no-age 指定 + プロジェクト設定なし → age 制約なし"
        );
    }

    #[test]
    fn test_build_filter_no_age_still_obeys_project_settings() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();

        // --no-age を指定してもプロジェクト設定が優先される
        let args = make_args_with_path(dir.path(), &["--no-age"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(10 * 24 * 60 * 60)
        );
    }

    #[test]
    fn test_resolved_min_age_matches_build_filter_age() {
        // install フェーズ (resolved_min_age) と judge フェーズ (build_filter) の age が
        // 常に一致することを保証する。direct deps と install 後の transitive deps で
        // age ポリシーを揃えるための回帰防止テスト。
        let dir = TempDir::new().unwrap();
        let args = make_args_with_path(dir.path(), &["--age", "2w"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        assert_eq!(
            orchestrator.resolved_min_age(),
            orchestrator.build_filter().min_age
        );
    }

    #[test]
    fn test_resolved_min_age_falls_back_to_default_without_cli_age() {
        // CLI --age 未指定でも install フェーズには組み込みデフォルト (1w) が反映される。
        // 修正前は install フェーズが生の args.age=None を見ており、CLI 未指定時に
        // transitive 依存へ age が効かない不整合があった。
        let dir = TempDir::new().unwrap();
        let args = make_args_with_path(dir.path(), &[]);
        let orchestrator = Orchestrator::new(args).unwrap();
        assert_eq!(
            orchestrator.resolved_min_age(),
            Some(crate::global_config::DEFAULT_AGE)
        );
    }

    #[test]
    fn test_resolved_min_age_none_with_no_age_and_no_project_settings() {
        // --no-age 指定かつプロジェクト設定が無い場合は install フェーズも age なし。
        let dir = TempDir::new().unwrap();
        let args = make_args_with_path(dir.path(), &["--no-age"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        assert_eq!(orchestrator.resolved_min_age(), None);
    }

    #[test]
    fn test_resolved_min_age_obeys_project_settings() {
        // プロジェクト minimumReleaseAge は CLI 未指定でも install フェーズへ反映される。
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();
        let args = make_args_with_path(dir.path(), &[]);
        let orchestrator = Orchestrator::new(args).unwrap();
        assert_eq!(
            orchestrator.resolved_min_age(),
            Some(std::time::Duration::from_secs(10 * 24 * 60 * 60))
        );
    }

    #[tokio::test]
    async fn test_version_cache_prevents_duplicate_fetches() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();

        // 既知のパッケージでキャッシュを事前に設定
        let cache_key = (Language::Node, "lodash".to_string());
        {
            let mut cache = orchestrator.version_cache.lock().await;
            cache.insert(
                cache_key,
                vec![VersionInfo {
                    version: "4.17.21".to_string(),
                    released_at: chrono::Utc::now(),
                    publisher: Default::default(),
                }],
            );
        }

        // 同じパッケージをフェッチ — ネットワークアクセスなしでキャッシュ結果を返すべき
        let adapter = orchestrator.get_adapter(Language::Node);
        let result = orchestrator.fetch_versions(&*adapter, "lodash").await;

        assert!(result.is_ok());
        let versions = result.unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, "4.17.21");
    }

    #[tokio::test]
    async fn test_version_cache_coalesces_concurrent_fetches() {
        let orchestrator = Orchestrator::new(make_args(&["depup"])).unwrap();
        let adapter = CountingAdapter {
            calls: AtomicUsize::new(0),
        };

        let (first, second, third) = tokio::join!(
            orchestrator.fetch_versions(&adapter, "shared-package"),
            orchestrator.fetch_versions(&adapter, "shared-package"),
            orchestrator.fetch_versions(&adapter, "shared-package")
        );

        assert!(first.is_ok());
        assert!(second.is_ok());
        assert!(third.is_ok());
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_run_directories_with_root_included() {
        let dir = TempDir::new().unwrap();

        // ルートマニフェストを作成
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        // マニフェスト付きのサブディレクトリを作成
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(
            dir.path().join("sub").join("Cargo.toml"),
            "[package]\nname = \"sub\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        // DepupConfig::directories_with_root を使ってディレクトリリストを構築
        let config = crate::config::DepupConfig {
            directories: vec![dir.path().join("sub")],
        };
        let dirs = config.directories_with_root(dir.path());

        assert_eq!(dirs.len(), 2);

        // これらのディレクトリでオーケストレータを実行 (ドライラン、ネットワークなし)
        let args = make_args_with_path(dir.path(), &["--dry-run"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let result = orchestrator.run_directories(&dirs).await;

        // 両ディレクトリのマニフェストが検出されるべき
        // (依存関係がないので更新は0件だが、エラーもないはず)
        assert!(result.errors.is_empty());
    }
}

/// Cargo.lock の age 監査を、偽の crates.io (local-registry 形式) と実 cargo で確かめる。
///
/// 公開日は固定の基準日時からの相対で与えるので、crates.io の状態にも実行日にも依存しない。
/// cargo はテスト専用の CARGO_HOME とオフラインで起動する (利用者の設定とネットワークに触れない)
#[cfg(test)]
mod lock_age_audit_tests {
    use super::*;
    use crate::test_support::fake_crates_io::FakeCratesIo;
    use crate::test_support::local_registry::{LocalRegistry, TestProject};
    use chrono::{DateTime, TimeZone, Utc};
    use clap::Parser;

    #[tokio::test]
    async fn verified_direct_and_transitive_releases_survive_final_lock_audit() {
        let registry = LocalRegistry::new();
        for version in ["1.0.0", "1.0.1"] {
            registry.publish("trusted-child", version, &[]);
            registry.publish("trusted-parent", version, &[("trusted-child", version)]);
            registry.publish("ordinary", version, &[]);
        }
        let project = TestProject::new(
            &registry,
            &manifest("trusted-parent = '=1.0.0'\nordinary = '=1.0.0'\n"),
        );
        let unpinned = manifest("trusted-parent = '1.0.0'\nordinary = '1.0.0'\n");
        let baseline = install(&project, &unpinned);
        let mut dates = FakeCratesIo::new();
        for name in ["trusted-parent", "trusted-child", "ordinary"] {
            dates = dates
                .release(name, "1.0.0", old())
                .release(name, "1.0.1", fresh());
        }
        for name in ["trusted-parent", "trusted-child"] {
            dates = dates.publisher(
                name,
                "1.0.1",
                crate::update::PublisherEvidence::GithubTrustedPublisher {
                    owner: "example-dev".into(),
                },
            );
        }
        let config = crate::global_config::GlobalConfig {
            age: Some("2w".into()),
            age_exempt: crate::update::AgeExemptions {
                github: vec!["example-dev".into()],
            },
            ..Default::default()
        };
        let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"]))
            .unwrap()
            .with_global_config(Some(config));
        let cargo = cargo_for(&project);
        let result = orchestrator
            .audit_lock_age(
                &LockAgeAudit {
                    project_dir: project.path(),
                    cutoff: cutoff(),
                    exemptions: &orchestrator.resolved_age_exemptions(),
                    baseline: &baseline,
                    preferred: &PreferredVersions::new(),
                    adapter: &dates,
                    cargo: &cargo,
                    budget: LOCK_AGE_AUDIT_BUDGET,
                },
                None,
            )
            .await;
        assert_eq!(project.locked_versions("trusted-parent"), ["1.0.1"]);
        assert_eq!(project.locked_versions("trusted-child"), ["1.0.1"]);
        assert_eq!(project.locked_versions("ordinary"), ["1.0.0"]);
        assert_eq!(result.unchecked, 0);
        assert_eq!(result.adjustments.len(), 1);
        assert_eq!(project.read("Cargo.toml"), unpinned);
        assert!(lock_is_accepted(&project));
    }

    /// この日時より後に公開された版を違反とする
    fn cutoff() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 16, 0, 0, 0).unwrap()
    }

    /// 期間を十分に満たす版の公開日
    fn old() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap()
    }

    /// 期間を満たす版の公開日 (基準日時の少し前)
    fn mature() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 4, 0, 0, 0).unwrap()
    }

    /// 期間を満たさない版の公開日
    fn fresh() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 25, 0, 0, 0).unwrap()
    }

    fn manifest(dependencies: &str) -> String {
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependencies}"
        )
    }

    fn cargo_for(project: &TestProject) -> CargoCommand {
        project.cargo_envs().into_iter().fold(
            CargoCommand::new().program(TestProject::cargo_program()),
            |cargo, (key, value)| cargo.env(key, value),
        )
    }

    /// 2 つの頂点 (fam-a / fam-b) が fam-core を `=` で固定し合う一族を公開する。
    /// wasm-bindgen 一族 (web-sys と wasm-bindgen-test がどちらも wasm-bindgen を `=` で掴む) と同じ形
    fn publish_family(registry: &LocalRegistry) {
        for version in ["1.0.0", "1.0.1", "1.0.2"] {
            let exact = format!("={version}");
            registry.publish("fam-core", version, &[]);
            registry.publish("fam-a", version, &[("fam-core", exact.as_str())]);
            registry.publish("fam-b", version, &[("fam-core", exact.as_str())]);
        }
    }

    /// 一族の公開日: 1.0.0 は古く、1.0.1 は期間を満たし、1.0.2 は新しすぎる
    fn with_family_dates(fake: FakeCratesIo) -> FakeCratesIo {
        ["fam-core", "fam-a", "fam-b"]
            .into_iter()
            .fold(fake, |fake, name| {
                fake.release(name, "1.0.0", old())
                    .release(name, "1.0.1", mature())
                    .release(name, "1.0.2", fresh())
            })
    }

    /// `=` で古い版に固定した manifest で lock を作ってから `unpinned` へ戻し、
    /// `--install` と同じ `cargo update` を実行する。install 前の lock を返す
    fn install(project: &TestProject, unpinned: &str) -> RegistryLockEntries {
        project.cargo_ok(&["generate-lockfile"]);
        project.write("Cargo.toml", unpinned);
        let baseline = parse_registry_entries(&project.read("Cargo.lock"));
        project.cargo_ok(&["update"]);
        baseline
    }

    async fn audit_with(
        project: &TestProject,
        baseline: &RegistryLockEntries,
        preferred: &PreferredVersions,
        adapter: &FakeCratesIo,
        budget: Duration,
    ) -> LockAgeAuditResult {
        let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
        let cargo = cargo_for(project);
        let audit = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &crate::update::AgeExemptions::default(),
            baseline,
            preferred,
            adapter,
            cargo: &cargo,
            budget,
        };
        orchestrator.audit_lock_age(&audit, None).await
    }

    async fn audit(
        project: &TestProject,
        baseline: &RegistryLockEntries,
        adapter: &FakeCratesIo,
    ) -> LockAgeAuditResult {
        audit_with(
            project,
            baseline,
            &PreferredVersions::new(),
            adapter,
            LOCK_AGE_AUDIT_BUDGET,
        )
        .await
    }

    fn adjustment<'a>(result: &'a LockAgeAuditResult, name: &str) -> &'a LockAgeAdjustment {
        result
            .adjustments
            .iter()
            .find(|adjustment| adjustment.name == name)
            .unwrap_or_else(|| panic!("{name} is not in {:?}", result.adjustments))
    }

    /// 元の manifest (`=` の固定を持たない) がそのまま lock を受け入れるか
    fn lock_is_accepted(project: &TestProject) -> bool {
        project.cargo(&["update", "--workspace", "--locked"]).0
    }

    /// 頂点が 2 つある `=` 一族は 1 件ずつでは戻せないが、まとめて解き直して戻す。
    /// 利用者の Cargo.toml は書き換えず、元の manifest で `--locked` が通る
    #[tokio::test]
    async fn test_family_with_two_tops_is_rolled_back_together() {
        let registry = LocalRegistry::new();
        publish_family(&registry);
        let project = TestProject::new(
            &registry,
            &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
        );
        let unpinned = manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\n");
        let baseline = install(&project, &unpinned);
        assert_eq!(project.locked_versions("fam-core"), vec!["1.0.2"]);

        let dates = with_family_dates(FakeCratesIo::new());
        let result = audit(&project, &baseline, &dates).await;

        for name in ["fam-a", "fam-b", "fam-core"] {
            assert_eq!(project.locked_versions(name), vec!["1.0.1"], "{name}");
            let adjustment = adjustment(&result, name);
            assert_eq!(adjustment.status, LockAgeStatus::Downgraded, "{name}");
            assert_eq!(adjustment.to.as_deref(), Some("1.0.1"), "{name}");
        }
        assert_eq!(result.unchecked, 0);
        assert_eq!(project.read("Cargo.toml"), unpinned);
        assert!(lock_is_accepted(&project));
    }

    /// 直接依存を経由しない (推移依存だけの) 一族も、まとめて解き直して戻す
    #[tokio::test]
    async fn test_transitive_only_family_is_rolled_back_together() {
        let registry = LocalRegistry::new();
        publish_family(&registry);
        registry.publish("wrap-x", "1.0.0", &[("fam-a", "1.0.0")]);
        registry.publish("wrap-y", "1.0.0", &[("fam-b", "1.0.0")]);
        let project = TestProject::new(
            &registry,
            &manifest(
                "wrap-x = \"1.0.0\"\nwrap-y = \"1.0.0\"\nfam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n",
            ),
        );
        let unpinned = manifest("wrap-x = \"1.0.0\"\nwrap-y = \"1.0.0\"\n");
        let baseline = install(&project, &unpinned);
        assert_eq!(project.locked_versions("fam-core"), vec!["1.0.2"]);

        let dates = with_family_dates(FakeCratesIo::new())
            .release("wrap-x", "1.0.0", old())
            .release("wrap-y", "1.0.0", old());
        let result = audit(&project, &baseline, &dates).await;

        for name in ["fam-a", "fam-b", "fam-core"] {
            assert_eq!(project.locked_versions(name), vec!["1.0.1"], "{name}");
            assert_eq!(
                adjustment(&result, name).status,
                LockAgeStatus::Downgraded,
                "{name}"
            );
        }
        assert_eq!(project.read("Cargo.toml"), unpinned);
        assert!(lock_is_accepted(&project));
    }

    /// 1 件ずつの差し戻しで済む crate の結果は変わらない (一族と同じ lock にあっても)
    #[tokio::test]
    async fn test_independent_crate_is_still_rolled_back_alone() {
        let registry = LocalRegistry::new();
        publish_family(&registry);
        for version in ["1.0.0", "1.0.1", "1.0.2"] {
            registry.publish("solo", version, &[]);
        }
        let project = TestProject::new(
            &registry,
            &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\nsolo = \"=1.0.0\"\n"),
        );
        let baseline = install(
            &project,
            &manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\nsolo = \"1.0.0\"\n"),
        );

        let dates = with_family_dates(FakeCratesIo::new())
            .release("solo", "1.0.0", old())
            .release("solo", "1.0.1", mature())
            .release("solo", "1.0.2", fresh());
        let result = audit(&project, &baseline, &dates).await;

        assert_eq!(project.locked_versions("solo"), vec!["1.0.1"]);
        let solo = adjustment(&result, "solo");
        assert_eq!(solo.status, LockAgeStatus::Downgraded);
        assert_eq!(solo.to.as_deref(), Some("1.0.1"));
        assert_eq!(project.locked_versions("fam-a"), vec!["1.0.1"]);
        assert!(lock_is_accepted(&project));
    }

    /// judge がこの実行で一族の 1 つだけ古い版 (OSV で新しい版を退けた等) を選んだ場合も、
    /// その版を差し戻し先にし、残りの一族を噛み合う版へ揃える
    #[tokio::test]
    async fn test_judge_choice_is_used_as_rollback_target() {
        let registry = LocalRegistry::new();
        publish_family(&registry);
        let project = TestProject::new(
            &registry,
            &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
        );
        let baseline = install(
            &project,
            &manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\n"),
        );

        let dates = with_family_dates(FakeCratesIo::new());
        let mut preferred = PreferredVersions::new();
        preferred.insert("fam-a".to_string(), vec!["1.0.0".to_string()]);
        let result = audit_with(
            &project,
            &baseline,
            &preferred,
            &dates,
            LOCK_AGE_AUDIT_BUDGET,
        )
        .await;

        for name in ["fam-a", "fam-b", "fam-core"] {
            assert_eq!(project.locked_versions(name), vec!["1.0.0"], "{name}");
            assert_eq!(
                adjustment(&result, name).status,
                LockAgeStatus::Downgraded,
                "{name}"
            );
        }
        assert!(lock_is_accepted(&project));
    }

    /// Cargo.toml 自身が新しすぎる版を要求している crate は戻せない理由を示し、
    /// それが同じ lock の一族のまとめ解きを巻き添えにしない
    #[tokio::test]
    async fn test_manifest_requirement_blocks_only_its_own_crate() {
        let registry = LocalRegistry::new();
        publish_family(&registry);
        registry.publish("blocker", "1.0.0", &[]);
        registry.publish("blocker", "1.0.1", &[]);
        let project = TestProject::new(
            &registry,
            &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
        );
        let unpinned = manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\nblocker = \"1.0.1\"\n");
        let baseline = install(&project, &unpinned);
        assert_eq!(project.locked_versions("blocker"), vec!["1.0.1"]);

        let dates = with_family_dates(FakeCratesIo::new())
            .release("blocker", "1.0.0", old())
            .release("blocker", "1.0.1", fresh());
        let result = audit(&project, &baseline, &dates).await;

        assert_eq!(
            adjustment(&result, "blocker").status,
            LockAgeStatus::BlockedByManifest("^1.0.1".to_string())
        );
        assert_eq!(project.locked_versions("blocker"), vec!["1.0.1"]);
        for name in ["fam-a", "fam-b", "fam-core"] {
            assert_eq!(project.locked_versions(name), vec!["1.0.1"], "{name}");
        }
        assert_eq!(project.read("Cargo.toml"), unpinned);
        assert!(lock_is_accepted(&project));
    }

    /// まとめ解きで新しく lock に入った crate も監査し、新しすぎれば差し戻す
    #[tokio::test]
    async fn test_crate_added_by_resolving_together_is_audited() {
        let registry = LocalRegistry::new();
        registry.publish("helper", "1.0.0", &[]);
        registry.publish("helper", "1.0.1", &[]);
        for version in ["1.0.0", "1.0.1", "1.0.2"] {
            let exact = format!("={version}");
            // 期間を満たす 1.0.1 だけが helper を必要とする
            if version == "1.0.1" {
                registry.publish("fam-core", version, &[("helper", "1.0.0")]);
            } else {
                registry.publish("fam-core", version, &[]);
            }
            registry.publish("fam-a", version, &[("fam-core", exact.as_str())]);
            registry.publish("fam-b", version, &[("fam-core", exact.as_str())]);
        }
        let project = TestProject::new(
            &registry,
            &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
        );
        let baseline = install(
            &project,
            &manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\n"),
        );
        assert!(project.locked_versions("helper").is_empty());

        let dates = with_family_dates(FakeCratesIo::new())
            .release("helper", "1.0.0", old())
            .release("helper", "1.0.1", fresh());
        let result = audit(&project, &baseline, &dates).await;

        assert_eq!(project.locked_versions("fam-core"), vec!["1.0.1"]);
        assert_eq!(project.locked_versions("helper"), vec!["1.0.0"]);
        assert_eq!(
            adjustment(&result, "helper").status,
            LockAgeStatus::Downgraded
        );
        assert!(lock_is_accepted(&project));
    }

    /// workspace (virtual manifest + member) でも写しでまとめて解き直し、member の
    /// Cargo.toml は書き換えない
    #[tokio::test]
    async fn test_workspace_members_are_rolled_back_together() {
        let registry = LocalRegistry::new();
        publish_family(&registry);
        let project = TestProject::new(
            &registry,
            "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"2\"\n",
        );
        let member = |name: &str, dependency: &str| {
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependency}\n"
            )
        };
        project.write("crates/a/Cargo.toml", &member("a", "fam-a = \"=1.0.0\""));
        project.write("crates/a/src/lib.rs", "");
        project.write("crates/b/Cargo.toml", &member("b", "fam-b = \"=1.0.0\""));
        project.write("crates/b/src/lib.rs", "");
        project.cargo_ok(&["generate-lockfile"]);
        let member_a = member("a", "fam-a = \"1.0.0\"");
        let member_b = member("b", "fam-b = \"1.0.0\"");
        project.write("crates/a/Cargo.toml", &member_a);
        project.write("crates/b/Cargo.toml", &member_b);
        let baseline = parse_registry_entries(&project.read("Cargo.lock"));
        project.cargo_ok(&["update"]);
        assert_eq!(project.locked_versions("fam-core"), vec!["1.0.2"]);

        let dates = with_family_dates(FakeCratesIo::new());
        let result = audit(&project, &baseline, &dates).await;

        for name in ["fam-a", "fam-b", "fam-core"] {
            assert_eq!(project.locked_versions(name), vec!["1.0.1"], "{name}");
            assert_eq!(
                adjustment(&result, name).status,
                LockAgeStatus::Downgraded,
                "{name}"
            );
        }
        assert_eq!(project.read("crates/a/Cargo.toml"), member_a);
        assert_eq!(project.read("crates/b/Cargo.toml"), member_b);
        assert!(lock_is_accepted(&project));
    }

    /// install 前から期間内の版が入っていた crate は、それより古くせず install 前の版へ戻し、
    /// 「期間を満たすよう差し戻した」とは区別して報告する
    #[tokio::test]
    async fn test_crate_returns_to_young_version_locked_before_install() {
        let registry = LocalRegistry::new();
        for version in ["1.0.0", "1.0.1", "1.0.2"] {
            registry.publish("solo", version, &[]);
        }
        let project = TestProject::new(&registry, &manifest("solo = \"=1.0.1\"\n"));
        let baseline = install(&project, &manifest("solo = \"1.0.1\"\n"));
        assert_eq!(project.locked_versions("solo"), vec!["1.0.2"]);

        let dates = FakeCratesIo::new()
            .release("solo", "1.0.0", old())
            .release(
                "solo",
                "1.0.1",
                Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap(),
            )
            .release("solo", "1.0.2", fresh());
        let result = audit(&project, &baseline, &dates).await;

        assert_eq!(project.locked_versions("solo"), vec!["1.0.1"]);
        let solo = adjustment(&result, "solo");
        assert_eq!(solo.status, LockAgeStatus::Restored);
        assert_eq!(solo.to.as_deref(), Some("1.0.1"));
        assert!(lock_is_accepted(&project));
    }

    /// 期間を満たす古い版が 1 つも無い crate は、差し戻せないものとして報告する
    #[tokio::test]
    async fn test_crate_without_older_mature_version_is_reported() {
        let registry = LocalRegistry::new();
        registry.publish("young", "0.1.0", &[]);
        let project = TestProject::new(&registry, &manifest(""));
        let unpinned = manifest("young = \"0.1.0\"\n");
        let baseline = install(&project, &unpinned);

        let dates = FakeCratesIo::new().release("young", "0.1.0", fresh());
        let result = audit(&project, &baseline, &dates).await;

        assert_eq!(
            adjustment(&result, "young").status,
            LockAgeStatus::NoOlderCandidate
        );
        assert_eq!(project.locked_versions("young"), vec!["0.1.0"]);
    }

    /// `--precise` が何も変えずに成功を返す cargo (directory source への置き換えと同じ振る舞い)
    /// でも、lock を読み直して「差し戻した」と誤報告せず、まとめ解きで実際に戻す
    #[cfg(unix)]
    #[tokio::test]
    async fn test_precise_that_changes_nothing_is_not_reported_as_rollback() {
        use std::os::unix::fs::PermissionsExt;

        let registry = LocalRegistry::new();
        for version in ["1.0.0", "1.0.1", "1.0.2"] {
            registry.publish("solo", version, &[]);
        }
        let project = TestProject::new(&registry, &manifest("solo = \"=1.0.0\"\n"));
        let baseline = install(&project, &manifest("solo = \"1.0.0\"\n"));
        assert_eq!(project.locked_versions("solo"), vec!["1.0.2"]);

        // `--precise` を含む起動だけ何もせずに exit 0 を返し、それ以外は本物の cargo へ渡す
        let stand_in = tempfile::TempDir::new().unwrap();
        let script = stand_in.path().join("cargo");
        std::fs::write(
            &script,
            "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"--precise\" ]; then exit 0; fi\ndone\nexec \"$DEPUP_TEST_REAL_CARGO\" \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cargo = project
            .cargo_envs()
            .into_iter()
            .fold(
                CargoCommand::new().program(&script),
                |cargo, (key, value)| cargo.env(key, value),
            )
            .env("DEPUP_TEST_REAL_CARGO", TestProject::cargo_program());

        let dates = FakeCratesIo::new()
            .release("solo", "1.0.0", old())
            .release("solo", "1.0.1", mature())
            .release("solo", "1.0.2", fresh());
        let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
        let audit = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &crate::update::AgeExemptions::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: LOCK_AGE_AUDIT_BUDGET,
        };
        let result = orchestrator.audit_lock_age(&audit, None).await;

        assert_eq!(project.locked_versions("solo"), vec!["1.0.1"]);
        let solo = adjustment(&result, "solo");
        assert_eq!(solo.status, LockAgeStatus::Downgraded);
        assert_eq!(solo.to.as_deref(), Some("1.0.1"));
        assert!(lock_is_accepted(&project));
    }

    /// まとめ解きの結果を元の manifest が受け入れず、しかも元の lock へ戻せなかったときは、
    /// 旧内容を退避した場所を `problems` で必ず知らせる (黙って壊れた lock を残さない)
    #[cfg(unix)]
    #[tokio::test]
    async fn test_failed_restore_is_reported_with_backup() {
        use std::os::unix::fs::PermissionsExt;

        // root では書き込み禁止が効かず、この経路を再現できない
        let probe = tempfile::NamedTempFile::new().unwrap();
        std::fs::set_permissions(probe.path(), std::fs::Permissions::from_mode(0o444)).unwrap();
        if std::fs::OpenOptions::new()
            .write(true)
            .open(probe.path())
            .is_ok()
        {
            return;
        }

        let registry = LocalRegistry::new();
        publish_family(&registry);
        let project = TestProject::new(
            &registry,
            &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
        );
        let baseline = install(
            &project,
            &manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\n"),
        );
        let before = project.read("Cargo.lock");

        // 元の manifest での検証 (`--locked`) だけを失敗させ、その前に Cargo.lock を
        // 書き込み禁止にして、元へ戻す書き込みも失敗させる
        let stand_in = tempfile::TempDir::new().unwrap();
        let script = stand_in.path().join("cargo");
        std::fs::write(
            &script,
            "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"--locked\" ]; then\n    chmod a-w Cargo.lock\n    echo 'error: simulated rejection' >&2\n    exit 1\n  fi\ndone\nexec \"$DEPUP_TEST_REAL_CARGO\" \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cargo = project
            .cargo_envs()
            .into_iter()
            .fold(
                CargoCommand::new().program(&script),
                |cargo, (key, value)| cargo.env(key, value),
            )
            .env("DEPUP_TEST_REAL_CARGO", TestProject::cargo_program());

        let dates = with_family_dates(FakeCratesIo::new());
        let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
        let audit = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &crate::update::AgeExemptions::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: LOCK_AGE_AUDIT_BUDGET,
        };
        let result = orchestrator.audit_lock_age(&audit, None).await;
        let lock_path = project.path().join("Cargo.lock");
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(result.problems.len(), 1, "{:?}", result.problems);
        let problem = &result.problems[0];
        assert!(problem.contains("simulated rejection"), "{problem}");
        assert!(problem.contains("restoring the previous"), "{problem}");
        let backup = problem
            .split("saved to ")
            .nth(1)
            .and_then(|rest| rest.split(" — ").next())
            .unwrap_or_else(|| panic!("no backup path in: {problem}"));
        assert_eq!(std::fs::read_to_string(backup).unwrap(), before);
        std::fs::remove_file(backup).unwrap();
    }

    /// 時間予算が尽きていれば何も変えず、監査できなかった件数を返す
    #[tokio::test]
    async fn test_exhausted_budget_leaves_everything_unchecked() {
        let registry = LocalRegistry::new();
        publish_family(&registry);
        let project = TestProject::new(
            &registry,
            &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
        );
        let baseline = install(
            &project,
            &manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\n"),
        );
        let before = project.read("Cargo.lock");

        let dates = with_family_dates(FakeCratesIo::new());
        let result = audit_with(
            &project,
            &baseline,
            &PreferredVersions::new(),
            &dates,
            Duration::ZERO,
        )
        .await;

        assert_eq!(result.unchecked, 3);
        assert!(result.adjustments.is_empty());
        assert_eq!(project.read("Cargo.lock"), before);
        assert_eq!(dates.fetch_count(), 0);
    }
}
