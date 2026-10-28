//! 更新オーケストレータ - 更新ワークフロー全体の調整
//!
//! このモジュールは以下を提供する:
//! - ワークフロー調整: 検出 → パース → フェッチ → 判定 → 書き込み
//! - レート制限付き並列レジストリクエリ
//! - ドライランモード対応
//! - 言語・パッケージフィルタの適用
//! - 部分的な継続を伴うエラーハンドリング

use crate::cargo_rollback::audit::{LockAgeAudit, LockAgeAuditor};
#[cfg(test)]
use crate::cargo_rollback::audit::{pick_older_within_age, rollback_target};
use crate::cargo_rollback::scratch::CargoCommand;
use crate::cli::CliArgs;
use crate::domain::{
    Dependency, GitReference, Language, ManifestUpdateResult, SkipReason, UpdateResult,
    UpdateSummary,
};
use crate::global_config::GlobalConfig;

mod age;
mod node_preflight;
mod osv_policy;
pub(crate) mod osv_report;
use crate::manifest::{
    ManifestInfo, ManifestWriter, MiseSettings, RegistryLockEntries, WriteResult, detect_manifests,
    find_cargo_lock_upward, get_parser, has_mise_config, read_git_entries,
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
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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

pub use crate::cargo_rollback::audit::{
    LOCK_AGE_AUDIT_BUDGET, LockAgeAdjustment, LockAgeAuditResult, LockAgeStatus, PreferredVersions,
};
pub use crate::registry::versions::VersionCache;

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
    /// crates.io の 1 リクエスト/秒 間隔を実行全体で共有する状態。
    /// アダプタごとに持たせるとマニフェスト境界やフェーズ境界で間隔がリセットされる
    crates_io_rate_limit: Arc<CratesIoRateLimit>,
    /// ディレクトリ・フェーズ間で共有するバージョン取得状態
    versions: crate::registry::versions::VersionFetcher,
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
    /// pnpm の最終的な解決結果が、表示した更新先へ到達しなかった。
    InstallResolutionError { directory: PathBuf, message: String },
    /// 書き込み前に停止した共有 pnpm scope。事後に同じ lock を再監査しない。
    NodePreflightError { directory: PathBuf, message: String },
    /// 判明していれば、Cargo マニフェストと確定した install・監査ルートを停止する。
    CargoPreflightError {
        manifest: PathBuf,
        root: Option<PathBuf>,
        message: String,
    },
    /// OSV 脆弱性チェックに失敗または脆弱性を検出
    OsvWarning { package: String, message: String },
    /// 脆弱な候補を除いた通知。実行を失敗させるエラーではない。
    OsvCandidateExcluded {
        package: String,
        version: String,
        advisories: Vec<String>,
    },
    OsvTradeoff {
        package: String,
        report: Box<crate::osv::RiskTradeoff>,
    },
}

impl OrchestratorError {
    pub fn is_warning(&self) -> bool {
        matches!(
            self,
            Self::OsvWarning { .. } | Self::OsvCandidateExcluded { .. } | Self::OsvTradeoff { .. }
        )
    }
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
            OrchestratorError::InstallResolutionError { directory, message } => {
                write!(
                    f,
                    "pnpm resolution verification failed in {}: {}",
                    directory.display(),
                    message
                )
            }
            OrchestratorError::NodePreflightError { directory, message } => {
                write!(
                    f,
                    "pnpm preflight failed in {}: {}",
                    directory.display(),
                    message
                )
            }
            OrchestratorError::CargoPreflightError {
                manifest, message, ..
            } => {
                write!(
                    f,
                    "Cargo preflight stopped {}: {}",
                    manifest.display(),
                    message
                )
            }
            OrchestratorError::OsvWarning { package, message } => {
                write!(f, "OSV check for {}: {}", package, message)
            }
            OrchestratorError::OsvCandidateExcluded {
                package,
                version,
                advisories,
            } => {
                write!(
                    f,
                    "OSV check for {package}: {version} vulnerable, falling back ({})",
                    if advisories.is_empty() {
                        "no advisory IDs".into()
                    } else {
                        advisories.join(", ")
                    }
                )
            }
            OrchestratorError::OsvTradeoff { package, report } => {
                write!(f, "OSV check for {package}: {}", report.describe(package))
            }
        }
    }
}

impl std::error::Error for OrchestratorError {}

impl Orchestrator {
    pub(crate) fn live_osv_details(&self) -> bool {
        self.args.verbose || self.args.quiet || self.args.json || self.args.diff
    }
    pub(crate) fn refresh_lock_requested(&self) -> bool {
        self.args.refresh_lock
    }

    pub(crate) async fn check_locked_version(
        &self,
        ecosystem: &str,
        name: &str,
        version: &str,
    ) -> Result<(), String> {
        let Some(checker) = &self.osv_checker else {
            return Ok(());
        };
        match checker.check(ecosystem, name, version).await? {
            OsvCheck::Safe => Ok(()),
            OsvCheck::Vulnerable(ids) => Err(format!(
                "{name}@{version} is vulnerable ({})",
                ids.join(", ")
            )),
        }
    }
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
            crates_io_rate_limit: Arc::new(CratesIoRateLimit::new()),
            versions: crate::registry::versions::VersionFetcher::new(
                DEFAULT_CONCURRENCY,
                CRATES_IO_CONCURRENCY,
            ),
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
    /// パスの最後にまとめて解き直す。
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
        self.enforce_lock_age_rust_tracked(project_dir, policy, baseline, preferred, bar)
            .await
            .0
    }

    pub(crate) async fn enforce_lock_age_rust_tracked(
        &self,
        project_dir: &Path,
        policy: &AgePolicy,
        baseline: &RegistryLockEntries,
        preferred: &PreferredVersions,
        bar: Option<&ProgressBar>,
    ) -> (
        LockAgeAuditResult,
        Option<crate::cargo_rollback::audit::AuditReceipt>,
    ) {
        let Some(cutoff) = policy.cutoff() else {
            return (LockAgeAuditResult::default(), None);
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
        let mut receipt = None;
        let result = LockAgeAuditor::new(&audit, &self.versions, self.args.verbose)
            .run_tracked(bar, &mut receipt)
            .await;
        (result, receipt)
    }

    /// 共有キャッシュを使って独立した lock 監査を実行する。
    #[cfg(test)]
    async fn audit_lock_age(
        &self,
        audit: &LockAgeAudit<'_>,
        bar: Option<&ProgressBar>,
    ) -> LockAgeAuditResult {
        LockAgeAuditor::new(audit, &self.versions, self.args.verbose)
            .run(bar)
            .await
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
        osv_baseline: Option<String>,
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
                let versions = match self
                    .resolve_java_release_dates(&dep, adapter, judge, versions)
                    .await
                {
                    Ok(versions) => versions,
                    Err(error) => {
                        return OnePassResult {
                            name: dep.name.clone(),
                            outcome: UpdateResult::skip_fetch_failed(dep, error.clone()),
                            fetch_error: Some(error),
                            osv_warnings: Vec::new(),
                        };
                    }
                };
                let versions = match self
                    .resolve_tag_release_dates(&dep, adapter, judge, versions)
                    .await
                {
                    Ok(versions) => versions,
                    Err(TagDateError::Unchecked(version)) => {
                        return OnePassResult {
                            name: dep.name.clone(),
                            outcome: UpdateResult::skip(
                                dep,
                                SkipReason::ReleaseDateUnchecked(version),
                            ),
                            fetch_error: None,
                            osv_warnings: Vec::new(),
                        };
                    }
                    Err(TagDateError::Failed(error)) => {
                        return OnePassResult {
                            name: dep.name.clone(),
                            outcome: UpdateResult::skip_fetch_failed(dep, error.clone()),
                            fetch_error: Some(error),
                            osv_warnings: Vec::new(),
                        };
                    }
                };
                // OSV チェック: judge で採用しようとした候補だけを問い合わせる。
                // 脆弱なら、その候補を除外して再 judge するループで安全な候補に
                // 自然にフォールバックする (1 依存あたり通常 1〜2 API call で済む)。
                // Swift など osv_ecosystem() == None の言語はスキップ。
                let mut osv_warnings = Vec::new();
                let result = match (self.osv_checker.as_ref(), dep.language.osv_ecosystem()) {
                    (Some(checker), Some(eco)) => {
                        let mut context = OsvJudgeContext {
                            checker,
                            ecosystem: eco,
                            bar,
                            warnings: &mut osv_warnings,
                            baseline: osv_baseline.as_deref(),
                        };
                        judge_with_osv(self, adapter, judge, &dep, versions, &mut context).await
                    }
                    _ => Ok(judge.judge(&dep, &versions)),
                };
                let result = match result {
                    Ok(result) => result,
                    Err(error) => {
                        return OnePassResult {
                            name: dep.name.clone(),
                            outcome: UpdateResult::skip_fetch_failed(dep, error.clone()),
                            fetch_error: Some(error),
                            osv_warnings,
                        };
                    }
                };
                let result = match result {
                    UpdateResult::Skip {
                        reason: SkipReason::AlreadyLatest,
                        released_at: Some(date),
                        ..
                    } if adapter.release_dates_deferred() && date.timestamp() == 0 => {
                        UpdateResult::skip_already_latest(dep.clone())
                    }
                    other => other,
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
        self.process_manifests_with_auditor(
            manifests,
            progress,
            &node_preflight::PnpmLockAuditor,
            &crate::cargo_scope::SystemCargoScopeResolver,
        )
        .await
    }

    async fn process_manifests_with_auditor(
        &self,
        manifests: &[ManifestInfo],
        progress: &mut Progress,
        auditor: &impl node_preflight::ExistingLockAuditor,
        resolver: &impl crate::cargo_scope::CargoScopeResolver,
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
        self.check_phase(parsed, progress, &mut summary, &mut errors, auditor)
            .await;
        self.sync_tauri_if_needed(manifests, progress, &mut summary, &mut errors)
            .await;
        let skip = if self.args.install && !self.args.dry_run {
            let cargo_boundary = self.rust_lock_boundary();
            let cargo_membership = crate::cargo_scope::CargoMembership::new(
                manifests
                    .iter()
                    .filter(|info| info.language == Language::Rust)
                    .map(|info| info.path.clone())
                    .chain(
                        summary
                            .manifests
                            .iter()
                            .filter(|result| result.language == Language::Rust)
                            .map(|result| result.path.clone()),
                    ),
                &cargo_boundary,
                resolver,
            );
            let scopes = node_preflight::ScopeContext {
                manifests,
                cargo: &cargo_membership,
            };
            progress.start(0, "Auditing existing pnpm-lock.yaml against --age");
            let bar = progress.bar();
            let blocked = node_preflight::preflight(
                &summary,
                &scopes,
                &errors,
                self.args.refresh_lock,
                |dir| self.resolved_age_policy_for_language(dir, Language::Node),
                auditor,
                bar.as_ref(),
            )
            .await;
            progress.finish_and_clear();
            node_preflight::block(&mut summary, &mut errors, &scopes, &blocked)
        } else {
            BTreeSet::new()
        };
        let write_results = self.write_phase(&mut summary, progress, &mut errors, &skip);

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
        auditor: &impl node_preflight::ExistingLockAuditor,
    ) {
        let total_deps: usize = parsed.iter().map(|p| p.dependencies.len()).sum();
        progress.start(total_deps as u64, "Checking dependencies");
        let progress_bar = progress.bar();

        let mut pnpm_settings = BTreeMap::new();
        for ParsedManifest { info, dependencies } in parsed {
            let osv_baselines = self.osv_baselines(info, &dependencies).await;
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
            let overrides = if info.language == Language::Node
                && dependencies
                    .iter()
                    .any(|dependency| filter.package_filter_skip_reason(dependency).is_none())
            {
                let scope = crate::node_scope::node_resolution_directory(&info.path);
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    pnpm_settings.entry(scope.clone())
                {
                    entry.insert(auditor.overrides(&scope).await);
                }
                match &pnpm_settings[&scope] {
                    Ok(settings) => Some(settings),
                    Err(message) => {
                        errors.push(OrchestratorError::ManifestParseError {
                            path: info.path.display().to_string(),
                            message: format!(
                                "could not verify effective pnpm overrides: {message}"
                            ),
                        });
                        continue;
                    }
                }
            } else {
                None
            };
            if filter.min_age.is_some()
                && !filter.age_exempt.is_empty()
                && !dependencies.is_empty()
                && !matches!(
                    info.language,
                    Language::Rust | Language::Go | Language::Swift
                )
            {
                progress.suspend(|| eprintln!(
                    "Notice: age_exempt.github is unavailable for other registries; their dependencies retain the age filter"
                ));
            }
            let package_filter = filter.clone();
            let judge = UpdateJudge::with_time(filter, self.evaluated_at);
            let judge = &judge;
            let mut manifest_result = ManifestUpdateResult::new(&info.path, info.language);
            // 複数 future から共有するため Arc に変換
            let adapter: Arc<dyn RegistryAdapter + Send + Sync> = if info.language == Language::Mise
            {
                Arc::new(MiseAdapter::for_project(dir, policy.cutoff()))
            } else if info.language == Language::Java {
                Arc::new(MavenCentralAdapter::for_manifest(
                    self.client.clone(),
                    &info.path,
                ))
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
                    let osv_baseline = osv_baselines.get(dep.manifest_name()).cloned();
                    let adapter = Arc::clone(&adapter);
                    let bar = progress_bar.clone();
                    let override_reason = overrides.and_then(|settings| settings.reason(&dep));
                    let package_filter = package_filter.package_filter_skip_reason(&dep);
                    async move {
                        if let Some(ref b) = bar {
                            b.set_message(format!("Checking {}", dep.name));
                        }
                        let result = if let Some(reason) =
                            package_filter.or_else(|| override_reason.map(SkipReason::PnpmOverride))
                        {
                            OnePassResult {
                                name: dep.name.clone(),
                                outcome: UpdateResult::skip(dep, reason),
                                fetch_error: None,
                                osv_warnings: Vec::new(),
                            }
                        } else {
                            self.process_one_dependency(
                                dep,
                                &*adapter,
                                judge,
                                bar.as_ref(),
                                osv_baseline,
                            )
                            .await
                        };
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
                errors.extend(result.osv_warnings);
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
        let projects: BTreeSet<_> = manifests
            .iter()
            .filter_map(|manifest| tauri_project_directory(manifest, manifests))
            .collect();
        if projects.is_empty() {
            return;
        }
        progress.spinner("Synchronizing Tauri versions...");
        for project_dir in projects {
            self.synchronize_tauri_versions(summary, errors, project_dir)
                .await;
        }
        progress.finish_and_clear();
    }

    /// 書き込み phase: 更新を適用 (`dry_run` ならプレビューのみ) し、書き込みエラーを集約する。
    ///
    /// 書き込めなかった更新は `summary` に記録する。以後の出力・install・lock の監査は、
    /// 書き込めた (dry-run では書き込める見込みの) 更新だけを更新として扱う
    fn write_phase(
        &self,
        summary: &mut UpdateSummary,
        progress: &mut Progress,
        errors: &mut Vec<OrchestratorError>,
        skip: &BTreeSet<PathBuf>,
    ) -> Vec<WriteResult> {
        if !self.args.dry_run {
            progress.spinner("Writing updates...");
        }
        let writer = ManifestWriter::new(self.args.dry_run);
        let writable: Vec<_> = summary
            .manifests
            .iter()
            .filter(|manifest| !skip.contains(&manifest.path))
            .cloned()
            .collect();
        let write_results = writer.apply_all_updates(&writable, get_parser);
        progress.finish_and_clear();

        for result in &write_results {
            for failure in &result.failed_updates {
                summary.record_write_error(&result.path, failure.index, failure.reason.clone());
            }
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
        if language == Language::Node {
            let root = age::project_scope_root(&self.args.path);
            let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            for scope in dir.ancestors().take_while(|scope| scope.starts_with(&root)) {
                if let Some((age, _)) =
                    crate::manifest::PnpmSettings::checked_minimum_release_age_with_source(scope)?
                {
                    policy.min_age = policy.min_age.max(Some(age));
                    policy.exemptions = crate::update::AgeExemptions::default();
                }
            }
        }
        // mise の利用者設定は --no-age で上書きする。プロジェクトの明示制約は残す。
        let include_user_settings = language != Language::Mise || !self.args.no_age;
        if let Some(age) = crate::manifest::native_age::native_min_age(
            language,
            dir,
            self.evaluated_at,
            include_user_settings,
        )? {
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

    /// チェック・同期・監査で共有する取得状態を使う。
    async fn fetch_versions(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> Result<Vec<VersionInfo>, String> {
        self.versions.fetch(adapter, package).await
    }

    /// GitHub のタグの一覧には公開日がない。age が有効なら、judge が選ぶ順 (新しい順) に
    /// 候補の公開日を取得し、age を満たす版に当たったら止める。公開日の無い版 (軽量タグ) は
    /// 日付のないまま残り、judge が候補から外す。問い合わせは 1 依存
    /// [`MAX_TAG_DATE_LOOKUPS`] 回まで (未認証の GitHub API は 1 時間 60 回。この実行で
    /// 解決済みの版は数えない)。上限に達したら、まだ確かめていない版を返す。
    async fn resolve_tag_release_dates(
        &self,
        dep: &Dependency,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        judge: &UpdateJudge,
        mut versions: Vec<VersionInfo>,
    ) -> Result<Vec<VersionInfo>, TagDateError> {
        if dep.language != Language::Swift || judge.age_cutoff().is_none() {
            return Ok(versions);
        }
        let candidates = judge.release_date_candidates(dep, &versions);
        let names = |list: Vec<&VersionInfo>| -> Vec<String> {
            list.into_iter().map(|info| info.version.clone()).collect()
        };
        let (within, beyond) = (names(candidates.within), names(candidates.beyond));
        let mut lookups = 0;
        for version in within {
            match self
                .check_tag_date(dep, adapter, judge, &mut versions, &version, &mut lookups)
                .await?
            {
                TagCheck::Admitted => return Ok(versions),
                TagCheck::Rejected => {}
                TagCheck::OutOfLookups => return Err(TagDateError::Unchecked(version)),
            }
        }
        // `--max-change` の内側に期間を満たす版が無い。外側に期間を満たす版があれば、judge は
        // ほかの言語と同じく `--max-change` で止まったと報告する (どちらでも更新はしない)
        for version in beyond {
            match self
                .check_tag_date(dep, adapter, judge, &mut versions, &version, &mut lookups)
                .await?
            {
                TagCheck::Admitted | TagCheck::OutOfLookups => break,
                TagCheck::Rejected => {}
            }
        }
        Ok(versions)
    }

    /// 候補 1 件の公開日を確かめる。この実行で解決していなければ問い合わせ、`lookups` を数える
    async fn check_tag_date(
        &self,
        dep: &Dependency,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        judge: &UpdateJudge,
        versions: &mut [VersionInfo],
        version: &str,
        lookups: &mut usize,
    ) -> Result<TagCheck, TagDateError> {
        let Some(info) = versions.iter_mut().find(|info| info.version == version) else {
            return Ok(TagCheck::Rejected);
        };
        // 公開者の例外なら、公開日を取らずに judge がこの版を選ぶ
        if judge.admits_age(dep, info) {
            return Ok(TagCheck::Admitted);
        }
        if info.released_at.is_none() {
            info.released_at = match self
                .versions
                .cached_release_date(adapter, &dep.name, version)
                .await
            {
                Some(date) => date,
                None if *lookups == MAX_TAG_DATE_LOOKUPS => return Ok(TagCheck::OutOfLookups),
                None => {
                    *lookups += 1;
                    self.versions
                        .release_date(adapter, &dep.name, version)
                        .await
                        .map_err(TagDateError::Failed)?
                }
            };
        }
        Ok(if judge.admits_age(dep, info) {
            TagCheck::Admitted
        } else {
            TagCheck::Rejected
        })
    }

    /// Maven のメタデータには版ごとの日時がない。judge が実際に選ぶ候補だけを解決し、
    /// POM の Last-Modified 日時で age フィルタを再評価する。
    async fn resolve_java_release_dates(
        &self,
        dep: &Dependency,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        judge: &UpdateJudge,
        mut versions: Vec<VersionInfo>,
    ) -> Result<Vec<VersionInfo>, String> {
        if !adapter.release_dates_deferred() || versions.is_empty() {
            return Ok(versions);
        }
        if dep.version_spec.kind == crate::domain::VersionSpecKind::Exact
            && !versions.iter().any(|candidate| {
                compare_dependency_versions(dep, &candidate.version, dep.version())
                    == std::cmp::Ordering::Equal
            })
        {
            return Err(format!(
                "current version {} is absent from Maven metadata",
                dep.version()
            ));
        }

        // 各周回で新しい候補を1つ解決する。min_age を満たさない候補は
        // 次の周回で除外し、古い候補をフォールバック先として残す。
        let mut confirmed_limited = false;
        for _ in 0..=versions.len() {
            let (selected, checking_limit) = match judge.judge(dep, &versions) {
                UpdateResult::Update { new_version, .. } => (Some(new_version), false),
                UpdateResult::Skip {
                    reason: SkipReason::AlreadyLatest,
                    ..
                } if self.args.verbose
                    && dep.version_spec.kind == crate::domain::VersionSpecKind::Exact =>
                {
                    (
                        versions
                            .iter()
                            .find(|candidate| candidate.version == dep.version())
                            .or_else(|| {
                                versions.iter().find(|candidate| {
                                    compare_dependency_versions(
                                        dep,
                                        &candidate.version,
                                        dep.version(),
                                    ) == std::cmp::Ordering::Equal
                                })
                            })
                            .map(|candidate| candidate.version.clone()),
                        false,
                    )
                }
                UpdateResult::Skip {
                    reason: SkipReason::ChangeLevelLimited(max),
                    ..
                } if !confirmed_limited => {
                    let candidate = judge
                        .candidates_before_age(dep, &versions)
                        .into_iter()
                        .filter(|info| {
                            info.released_at.is_none()
                                && compare_dependency_versions(dep, &info.version, dep.version())
                                    == std::cmp::Ordering::Greater
                                && crate::domain::ChangeLevel::from_versions(
                                    dep.version(),
                                    &info.version,
                                )
                                .is_some_and(|level| level > max)
                        })
                        .max_by(|a, b| compare_dependency_versions(dep, &a.version, &b.version))
                        .map(|info| info.version.clone());
                    (candidate, true)
                }
                _ => (None, false),
            };
            let Some(selected) = selected else {
                return Ok(versions);
            };
            let Some(info) = versions.iter_mut().find(|info| info.version == selected) else {
                return Err(format!(
                    "selected Maven version {selected} is absent from metadata"
                ));
            };
            if info.released_at.is_some() {
                return Ok(versions);
            }
            let date = self
                .versions
                .release_date(adapter, &dep.name, &selected)
                .await?
                .ok_or_else(|| format!("release date unavailable for {selected}"))?;
            info.released_at = Some(date);
            if checking_limit && judge.admits_age(dep, info) {
                confirmed_limited = true;
            }
        }
        Err("could not resolve Maven release dates".to_string())
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
                            .filter(|info| info.released_at.is_none_or(|date| date > cutoff))
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

/// Cargo workspace 経由の検出では flag が付かないため、対応する Node manifest も照合する。
fn tauri_project_directory<'a>(
    manifest: &'a ManifestInfo,
    inventory: &[ManifestInfo],
) -> Option<&'a Path> {
    if manifest.language != Language::Rust {
        return None;
    }
    let project = manifest.path.parent()?.parent()?;
    (manifest.is_tauri_rust
        || (manifest.path.ends_with("src-tauri/Cargo.toml")
            && inventory.iter().any(|candidate| {
                candidate.language == Language::Node
                    && candidate.path == project.join("package.json")
            })))
    .then_some(project)
}

/// 1 依存で、age の判定のためにタグの公開日を取得する回数の上限。
/// 1 回は Release、無ければタグの参照と注釈付きタグの最大 3 リクエストになる。
const MAX_TAG_DATE_LOOKUPS: usize = 5;

/// タグの公開日を揃えられなかった理由
enum TagDateError {
    /// 取得の上限に達し、この版とそれより古い候補の公開日を確かめていない
    Unchecked(String),
    /// 取得に失敗した (通信・レート制限・応答の誤り)
    Failed(String),
}

/// 候補 1 件の公開日を確かめた結果
enum TagCheck {
    /// 期間を満たす (または公開者の例外)。judge はこの版を選びうる
    Admitted,
    /// 期間内か、公開日が無い
    Rejected,
    /// 取得の上限に達し、確かめていない
    OutOfLookups,
}

/// バージョンチェック並列処理で 1 件の依存から得られた結果
/// (内部利用のみ)。
struct OnePassResult {
    name: String,
    outcome: UpdateResult,
    /// fetch 失敗時の原因メッセージ (存在する場合のみ `OrchestratorError` として記録される)
    fetch_error: Option<String>,
    /// OSV チェックで除外・問題が生じた version のメッセージ
    osv_warnings: Vec<OrchestratorError>,
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
/// 3. 候補を OSV で除外した後の `Skip` は、残した基準版と age 待ちの版を診断する。
///    OSV による除外が無ければ、通常の Skip をそのまま返す。
///
/// 通常 1 依存あたり 1〜2 API call で済む。
/// 全 candidate を網羅的にチェックする旧実装と違い、`@angular/*` のように
/// 1000+ バージョンを持つパッケージでも実用的な速度で完了する。
struct OsvJudgeContext<'a> {
    checker: &'a OsvChecker,
    ecosystem: &'a str,
    bar: Option<&'a ProgressBar>,
    warnings: &'a mut Vec<OrchestratorError>,
    baseline: Option<&'a str>,
}

struct OsvFallback {
    outcome: UpdateResult,
    report: crate::osv::RiskTradeoff,
    skipped: Vec<String>,
}

impl OsvFallback {
    fn finish(self, bar: Option<&ProgressBar>, name: &str, live_details: bool) -> UpdateResult {
        let message = format!("  ⚠ {}", self.report.describe(name));
        osv_println(bar, &message, live_details);
        self.outcome
            .with_osv_skipped(self.skipped)
            .with_osv_risk_tradeoff(self.report)
    }
}

async fn judge_with_osv(
    orchestrator: &Orchestrator,
    adapter: &(dyn RegistryAdapter + Send + Sync),
    judge: &UpdateJudge,
    dep: &Dependency,
    versions: Vec<VersionInfo>,
    context: &mut OsvJudgeContext<'_>,
) -> Result<UpdateResult, String> {
    let bar = context.bar;
    let warnings = &mut context.warnings;
    let checker = context.checker;
    let ecosystem = context.ecosystem;
    let live_details = orchestrator.live_osv_details();
    let warning = |message| OrchestratorError::OsvWarning {
        package: dep.name.clone(),
        message,
    };
    let mut allowed = versions;
    let mut locked_baseline_safe: Option<bool> = None;
    let mut fallback_chain: Vec<String> = Vec::new();
    let mut improvement: Option<OsvFallback> = None;
    let mut tradeoff: Option<OsvFallback> = None;
    loop {
        allowed = match orchestrator
            .resolve_java_release_dates(dep, adapter, judge, allowed)
            .await
        {
            Ok(versions) => versions,
            Err(error) => return Err(error),
        };
        let result = judge.judge(dep, &allowed);
        let UpdateResult::Update {
            new_version: target,
            ..
        } = &result
        else {
            if let Some(preferred) = improvement.take().or_else(|| tradeoff.take()) {
                return Ok(preferred.finish(bar, &dep.name, live_details));
            }
            if fallback_chain.is_empty() {
                return Ok(result);
            }
            // フォールバックを使い切ったことを、通常の「最新」と区別する。
            // 診断用の照会では age や他の更新条件を緩めて採用しない。
            let mut diagnostic = crate::domain::OsvBlocked {
                skipped_versions: fallback_chain,
                ..Default::default()
            };
            if let Some(current) = crate::update::current_version_info(dep, &allowed) {
                diagnostic.current_version = Some(current.version.clone());
                match checker.check(ecosystem, &dep.name, &current.version).await {
                    Ok(OsvCheck::Vulnerable(ids)) => {
                        diagnostic.current_vulnerable = true;
                        diagnostic.current_advisories = ids;
                    }
                    Ok(OsvCheck::Safe) => {}
                    Err(error) => diagnostic.current_check_error = Some(error),
                }
            } else {
                diagnostic.current_check_error = Some(
                    "manifest baseline could not be matched to a published version; not queried"
                        .into(),
                );
            }
            // 通常の更新では選べなかった新しい版だけを、新しい順に最大5件確認する。
            // 大量のリリースを持つ依存でも、診断が更新処理を際限なく延ばさない。
            let waiting = judge.age_waiting_candidates(dep, &allowed);
            for (info, eligible_at) in waiting.iter().take(5) {
                if let Some(b) = bar {
                    b.set_message(format!(
                        "OSV: {} {} (waiting for --age)",
                        dep.name, info.version
                    ));
                }
                match checker.check(ecosystem, &dep.name, &info.version).await {
                    Ok(OsvCheck::Safe) => {
                        diagnostic.safe_after_age = Some(crate::domain::AgeWaitingVersion {
                            version: info.version.clone(),
                            eligible_at: *eligible_at,
                        });
                        break;
                    }
                    Ok(OsvCheck::Vulnerable(_)) => {}
                    Err(error) => {
                        diagnostic.waiting_check_error = Some(error);
                        break;
                    }
                }
            }
            if diagnostic.safe_after_age.is_none()
                && diagnostic.waiting_check_error.is_none()
                && waiting.len() > 5
            {
                diagnostic.waiting_check_error = Some(
                    "OSV waiting-version lookup limit reached; older versions not checked".into(),
                );
            }
            let released_at = match result {
                UpdateResult::Skip { released_at, .. } => released_at,
                _ => None,
            };
            return Ok(UpdateResult::skip_with_optional_date(
                dep.clone(),
                SkipReason::KnownVulnerabilities(diagnostic),
                released_at,
            ));
        };
        let target = target.clone();

        // lock より古い版を書いても、解決済みの脆弱な版がそのまま残り得る。
        // lock が安全と確認できる場合は、従来どおり manifest の下限を上げられる。
        if let Some(baseline) = context.baseline
            && compare_dependency_versions(dep, &target, baseline) == std::cmp::Ordering::Less
        {
            let safe = match locked_baseline_safe {
                Some(safe) => safe,
                None => {
                    let safe = match checker.check(ecosystem, &dep.name, baseline).await {
                        Ok(OsvCheck::Safe) => true,
                        Ok(OsvCheck::Vulnerable(_)) => false,
                        Err(error) => {
                            warnings.push(warning(format!(
                                "locked baseline OSV check failed: {error}"
                            )));
                            false
                        }
                    };
                    locked_baseline_safe = Some(safe);
                    safe
                }
            };
            if !safe {
                fallback_chain.push(format!(
                    "{target} (below vulnerable or unconfirmed locked baseline {baseline})"
                ));
                let before = allowed.len();
                allowed.retain(|version| {
                    compare_dependency_versions(dep, &version.version, &target)
                        != std::cmp::Ordering::Equal
                });
                if before == allowed.len() {
                    if let Some(preferred) = improvement.take().or_else(|| tradeoff.take()) {
                        return Ok(preferred.finish(bar, &dep.name, live_details));
                    }
                    return Err(format!(
                        "could not exclude candidate {target} below locked baseline"
                    ));
                }
                continue;
            }
        }

        if let Some(b) = bar {
            b.set_message(format!("OSV: {} {}", dep.name, target));
        }

        let baseline: Vec<String> = context.baseline.into_iter().map(str::to_string).collect();
        match checker
            .assess_versions(
                ecosystem,
                &dep.name,
                &baseline,
                std::slice::from_ref(&target),
                orchestrator.osv_risk_tradeoff_enabled(),
            )
            .await
        {
            Ok(crate::osv::Assessment::Safe) => {
                if !fallback_chain.is_empty() {
                    let line = format!(
                        "  ↓ {}: skipped {} due to OSV → using {}",
                        dep.name,
                        fallback_chain.join(", "),
                        target
                    );
                    osv_println(bar, &line, live_details);
                    return Ok(result
                        .with_osv_skipped(fallback_chain)
                        .with_osv_checked(true));
                }
                // チェック完了・脆弱性なし
                return Ok(result.with_osv_checked(true));
            }
            Ok(crate::osv::Assessment::Tradeoff(report)) => {
                let slot = if report.introduced.is_empty() {
                    &mut improvement
                } else {
                    &mut tradeoff
                };
                if slot.is_none() {
                    *slot = Some(OsvFallback {
                        outcome: result,
                        report,
                        skipped: fallback_chain.clone(),
                    });
                }
                fallback_chain.push(format!(
                    "{target} (known vulnerabilities; trade-off candidate)"
                ));
                let before = allowed.len();
                allowed.retain(|version| {
                    compare_dependency_versions(dep, &version.version, &target)
                        != std::cmp::Ordering::Equal
                });
                if before == allowed.len() {
                    if let Some(preferred) = improvement.take().or_else(|| tradeoff.take()) {
                        return Ok(preferred.finish(bar, &dep.name, live_details));
                    }
                    return Err(format!(
                        "could not exclude {target} while looking for an advisory-free candidate"
                    ));
                }
            }
            Ok(crate::osv::Assessment::Rejected {
                advisories,
                baseline_error,
            }) => {
                if let Some(error) = baseline_error {
                    warnings.push(warning(format!(
                        "baseline OSV check failed; risk-reduction exception disabled: {error}"
                    )));
                }
                let ids: Vec<_> = advisories
                    .into_iter()
                    .map(|advisory| advisory.id)
                    .filter(|id| !id.is_empty())
                    .collect();
                let detail = if ids.is_empty() {
                    "no advisory IDs".to_string()
                } else {
                    ids.join(", ")
                };
                let line = format!("  ⚠ OSV: {} {} vulnerable ({})", dep.name, target, detail);
                osv_println(bar, &line, live_details);

                fallback_chain.push(format!("{} ({})", target, detail));
                warnings.push(OrchestratorError::OsvCandidateExcluded {
                    package: dep.name.clone(),
                    version: target.clone(),
                    advisories: ids,
                });
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
                        "  ⚠ {}: could not exclude {} from candidates, stopping candidate selection",
                        dep.name, target
                    );
                    osv_println(bar, &line, live_details);
                    warnings.push(warning(format!(
                        "could not exclude {} from candidates, stopping OSV check",
                        target
                    )));
                    if let Some(preferred) = improvement.take().or_else(|| tradeoff.take()) {
                        return Ok(preferred.finish(bar, &dep.name, live_details));
                    }
                    return Err(format!("could not exclude OSV-rejected candidate {target}"));
                }
                // ループ継続 → 次の候補で再判定
            }
            Err(e) => {
                let line = format!("  ⚠ OSV check failed for {} {}: {}", dep.name, target, e);
                osv_println(bar, &line, live_details);
                warnings.push(warning(format!("OSV check failed for {}: {}", target, e)));
                if let Some(preferred) = improvement.take().or_else(|| tradeoff.take()) {
                    return Ok(preferred.finish(bar, &dep.name, live_details));
                }
                return Ok(if fallback_chain.is_empty() {
                    result
                } else {
                    result.with_osv_skipped(fallback_chain)
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "orchestrator/osv_tests.rs"]
mod osv_tests;

/// 進捗バーを一時的に隠して診断を出す。非 TTY でも診断を保持する。
fn osv_println(bar: Option<&ProgressBar>, line: &str, enabled: bool) {
    if !enabled {
        return;
    }
    match bar {
        Some(b) => b.suspend(|| eprintln!("{line}")),
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
        UpdateResult::Update {
            osv_skipped,
            osv_risk_tradeoff,
            ..
        } => !osv_skipped.is_empty() || osv_risk_tradeoff.is_some(),
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
            SkipReason::CurrentVersionTooRecent,
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
        orchestrator.versions.cache.lock().await.insert(
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
    async fn write_failures_are_recorded_in_summary() {
        // 回帰 (#21): 書き込めなかった更新 (ここでは曖昧として拒否した更新) は summary に
        // 記録し、更新の件数に入れない。dry-run でも書き込める見込みかどうかで数える
        let root = TempDir::new().unwrap();
        fs::write(
            root.path().join("package.json"),
            r#"{"dependencies":{"shared":"^1.0.0","library":"^1.0.0"},"devDependencies":{"shared":"~1.0.0"}}"#,
        )
        .unwrap();
        let orchestrator = Orchestrator::new(make_args_with_path(
            root.path(),
            &["--no-age", "--no-osv", "--dry-run", "--quiet"],
        ))
        .unwrap();
        let now = chrono::Utc::now();
        {
            let mut cache = orchestrator.versions.cache.lock().await;
            for name in ["shared", "library"] {
                cache.insert(
                    (Language::Node, name.into()),
                    vec![
                        VersionInfo::new("1.0.0", now - chrono::Duration::days(60)),
                        VersionInfo::new("1.5.0", now - chrono::Duration::days(30)),
                    ],
                );
            }
        }

        let result = orchestrator.run().await;

        let manifest = &result.summary.manifests[0];
        assert_eq!(manifest.judged_updates().count(), 3);
        let updated: Vec<_> = manifest.updates().map(|u| u.package_name()).collect();
        assert_eq!(updated, vec!["library"]);
        let failed: Vec<_> = manifest
            .failed_updates()
            .map(|u| (u.package_name(), u.dependency().is_dev))
            .collect();
        assert_eq!(failed, vec![("shared", false), ("shared", true)]);
        assert!(manifest.failed_updates().all(|u| {
            u.write_error()
                .unwrap()
                .contains("ambiguous dependency 'shared'")
        }));
        assert_eq!(result.summary.total_updates(), 1);
        assert_eq!(result.summary.total_failed(), 2);
        // エラーの行は依存ごとに 1 行
        assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
        assert_eq!(result.write_results[0].updates_failed(), 2);
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
            let mut cache = orchestrator.versions.cache.lock().await;
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

    struct DatedMavenAdapter {
        calls: AtomicUsize,
        now: chrono::DateTime<chrono::Utc>,
    }

    #[async_trait]
    impl RegistryAdapter for DatedMavenAdapter {
        fn language(&self) -> Language {
            Language::Java
        }

        fn registry_name(&self) -> &'static str {
            "Maven Central"
        }

        fn release_dates_deferred(&self) -> bool {
            true
        }

        async fn fetch_versions(
            &self,
            _package: &str,
        ) -> Result<Vec<VersionInfo>, crate::error::RegistryError> {
            Ok(Vec::new())
        }

        async fn fetch_release_date(
            &self,
            _package: &str,
            version: &str,
        ) -> Result<Option<chrono::DateTime<chrono::Utc>>, crate::error::RegistryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let days = if matches!(version, "3.0.0" | "4.0.0") {
                1
            } else {
                30
            };
            Ok(Some(self.now - chrono::Duration::days(days)))
        }
    }

    #[tokio::test]
    async fn maven_dates_are_fetched_only_for_selected_candidates() {
        let dir = TempDir::new().unwrap();
        let orchestrator = Orchestrator::new(make_args_with_path(dir.path(), &["--java"])).unwrap();
        let now = chrono::Utc::now();
        let adapter = DatedMavenAdapter {
            calls: AtomicUsize::new(0),
            now,
        };
        let judge = UpdateJudge::with_time(
            UpdateFilter::new()
                .with_include_pinned(true)
                .with_min_age(Duration::from_secs(14 * 86400)),
            now,
        );
        let dep = Dependency::new(
            "example:library",
            crate::domain::VersionSpec::new(
                crate::domain::VersionSpecKind::Exact,
                "1.0.0",
                "1.0.0",
            ),
            false,
            Language::Java,
        );
        let versions = ["1.0.0", "2.0.0", "3.0.0"]
            .map(VersionInfo::undated)
            .to_vec();
        let resolved = orchestrator
            .resolve_java_release_dates(&dep, &adapter, &judge, versions.clone())
            .await
            .unwrap();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            resolved[1].released_at,
            Some(now - chrono::Duration::days(30))
        );
        assert_eq!(
            resolved[2].released_at,
            Some(now - chrono::Duration::days(1))
        );
        assert!(matches!(
            judge.judge(&dep, &resolved),
            UpdateResult::Update { new_version, .. } if new_version == "2.0.0"
        ));
        orchestrator
            .resolve_java_release_dates(&dep, &adapter, &judge, versions)
            .await
            .unwrap();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);

        let missing = vec![VersionInfo::undated("2.0.0")];
        assert!(
            orchestrator
                .resolve_java_release_dates(&dep, &adapter, &judge, missing)
                .await
                .unwrap_err()
                .contains("absent from Maven metadata")
        );
    }

    /// GitHub のタグの代わり。一覧には日付が無く、公開日は版ごとに返す
    /// (`None` は Release も注釈付きタグも無い版)。問い合わせた版を記録する。
    struct DatedTagsAdapter {
        dates: Vec<(&'static str, Option<i64>)>,
        now: chrono::DateTime<chrono::Utc>,
        lookups: std::sync::Mutex<Vec<String>>,
        fail: bool,
    }

    #[async_trait]
    impl RegistryAdapter for DatedTagsAdapter {
        fn language(&self) -> Language {
            Language::Swift
        }

        fn registry_name(&self) -> &'static str {
            "GitHub Tags"
        }

        async fn fetch_versions(
            &self,
            _package: &str,
        ) -> Result<Vec<VersionInfo>, crate::error::RegistryError> {
            Ok(self
                .dates
                .iter()
                .map(|(version, _)| VersionInfo::undated(*version))
                .collect())
        }

        async fn fetch_release_date(
            &self,
            _package: &str,
            version: &str,
        ) -> Result<Option<chrono::DateTime<chrono::Utc>>, crate::error::RegistryError> {
            self.lookups.lock().unwrap().push(version.to_string());
            if self.fail {
                return Err(crate::error::RegistryError::RateLimitExceeded {
                    registry: "GitHub Tags".into(),
                });
            }
            Ok(self
                .dates
                .iter()
                .find(|(candidate, _)| *candidate == version)
                .and_then(|(_, days)| *days)
                .map(|days| self.now - chrono::Duration::days(days)))
        }
    }

    /// `from: "1.0.0"` で宣言した GitHub の Swift の依存
    fn swift_dependency(name: &str) -> Dependency {
        Dependency::production(
            name,
            crate::domain::VersionSpec::new(
                crate::domain::VersionSpecKind::Caret,
                "1.0.0",
                "1.0.0",
            ),
            Language::Swift,
        )
    }

    /// Swift の公開日の取得の 1 組
    struct SwiftDateCase {
        /// 版と公開からの日数 (`None` は公開日の無い版)
        dates: Vec<(&'static str, Option<i64>)>,
        exempt: bool,
        age: bool,
        max_change: Option<crate::domain::ChangeLevel>,
        expected: &'static str,
        /// 問い合わせる版 (順)
        lookups: Vec<&'static str>,
    }

    impl SwiftDateCase {
        fn new(dates: &[(&'static str, Option<i64>)], expected: &'static str) -> Self {
            Self {
                dates: dates.to_vec(),
                exempt: false,
                age: true,
                max_change: None,
                expected,
                lookups: Vec::new(),
            }
        }

        fn lookups(mut self, lookups: &[&'static str]) -> Self {
            self.lookups = lookups.to_vec();
            self
        }
    }

    /// Issue #59: Swift の依存は、age が有効なら judge が選ぶ順に公開日を取得し、
    /// 期間を満たす版に当たったら止める。公開日の無い版は候補にしない。
    #[tokio::test]
    async fn swift_release_dates_are_fetched_newest_first_until_one_passes_age() {
        let unknown: Vec<(&str, Option<i64>)> = ["1.1.0", "1.2.0", "1.3.0", "1.4.0", "1.5.0"]
            .into_iter()
            .map(|version| (version, None))
            .collect();
        let cases = vec![
            // 最新は 3 日前なので、期間を満たす次の版で止まる (それより古い版は問い合わせない)
            SwiftDateCase::new(
                &[("1.0.0", Some(90)), ("1.1.0", Some(30)), ("1.2.0", Some(3))],
                "update 1.1.0",
            )
            .lookups(&["1.2.0", "1.1.0"]),
            // 最新の公開日が分からなくても、期間を満たす次の版へ進む
            SwiftDateCase::new(
                &[("1.0.0", Some(90)), ("1.1.0", Some(30)), ("1.2.0", None)],
                "update 1.1.0",
            )
            .lookups(&["1.2.0", "1.1.0"]),
            // どれも公開日が分からない
            SwiftDateCase::new(
                &[("1.0.0", Some(90)), ("1.1.0", None), ("1.2.0", None)],
                "release date unavailable for 1.2.0",
            )
            .lookups(&["1.2.0", "1.1.0"]),
            // どれも期間内
            SwiftDateCase::new(
                &[("1.0.0", Some(90)), ("1.1.0", Some(2)), ("1.2.0", Some(1))],
                "no suitable version",
            )
            .lookups(&["1.2.0", "1.1.0"]),
            // 公開者の例外と age の無効では、公開日を取りに行かない
            SwiftDateCase {
                exempt: true,
                ..SwiftDateCase::new(&[("1.0.0", Some(90)), ("1.2.0", Some(1))], "update 1.2.0")
            },
            SwiftDateCase {
                age: false,
                ..SwiftDateCase::new(&[("1.0.0", Some(90)), ("1.2.0", Some(1))], "update 1.2.0")
            },
            // 上限の 5 回目で期間を満たす版に当たる
            SwiftDateCase::new(
                &[
                    ("1.0.0", Some(90)),
                    ("1.1.0", Some(30)),
                    ("1.2.0", None),
                    ("1.3.0", None),
                    ("1.4.0", None),
                    ("1.5.0", None),
                ],
                "update 1.1.0",
            )
            .lookups(&["1.5.0", "1.4.0", "1.3.0", "1.2.0", "1.1.0"]),
            // ちょうど 5 件の候補をすべて確かめ、どれも公開日が分からない
            SwiftDateCase::new(
                &[&[("1.0.0", Some(90))][..], &unknown].concat(),
                "release date unavailable for 1.5.0",
            )
            .lookups(&["1.5.0", "1.4.0", "1.3.0", "1.2.0", "1.1.0"]),
            // 取得の上限に達したら、確かめていない版を示して止める
            SwiftDateCase::new(
                &[
                    ("1.0.0", Some(90)),
                    ("1.1.0", Some(30)),
                    ("1.2.0", None),
                    ("1.3.0", None),
                    ("1.4.0", None),
                    ("1.5.0", None),
                    ("1.6.0", None),
                ],
                "release date lookup limit reached; 1.1.0 and older not checked",
            )
            .lookups(&["1.6.0", "1.5.0", "1.4.0", "1.3.0", "1.2.0"]),
            // `--max-change` の内側が期間内なら、外側の版を現在版に近い順に確かめ、
            // 期間を満たす版があれば `--max-change` で止まったと報告する
            SwiftDateCase {
                max_change: Some(crate::domain::ChangeLevel::Patch),
                ..SwiftDateCase::new(
                    &[
                        ("1.0.0", Some(90)),
                        ("1.0.1", Some(2)),
                        ("1.1.0", Some(30)),
                        ("2.0.0", Some(1)),
                    ],
                    "limited by --max-change=patch",
                )
                .lookups(&["1.0.1", "1.1.0"])
            },
        ];
        let dir = TempDir::new().unwrap();
        let orchestrator =
            Orchestrator::new(make_args_with_path(dir.path(), &["--swift"])).unwrap();
        let now = chrono::Utc::now();
        for (index, case) in cases.into_iter().enumerate() {
            let adapter = DatedTagsAdapter {
                dates: case.dates.clone(),
                now,
                lookups: std::sync::Mutex::new(Vec::new()),
                fail: false,
            };
            let mut filter = UpdateFilter::new();
            if case.age {
                filter = filter.with_min_age(Duration::from_secs(14 * 86400));
            }
            if case.exempt {
                filter.age_exempt.github = vec!["owner".into()];
            }
            if let Some(level) = case.max_change {
                filter = filter.with_max_change(level);
            }
            let judge = UpdateJudge::with_time(filter, now);
            // 版と公開日のキャッシュは実行全体で共有されるので、組ごとに名前を変える
            let dep = swift_dependency(&format!("owner/library{index}"));
            let result = orchestrator
                .process_one_dependency(dep, &adapter, &judge, None, None)
                .await
                .outcome;
            let outcome = match &result {
                UpdateResult::Update { new_version, .. } => format!("update {new_version}"),
                UpdateResult::Skip { reason, .. } => reason.to_string(),
            };
            assert_eq!(outcome, case.expected, "{:?}", case.dates);
            assert_eq!(
                *adapter.lookups.lock().unwrap(),
                case.lookups,
                "{:?} {}",
                case.dates,
                case.expected
            );
            // 選んだ版の公開日を表示に使う
            if let UpdateResult::Update {
                released_at,
                new_version,
                ..
            } = &result
                && case.age
                && !case.exempt
            {
                let days = case
                    .dates
                    .iter()
                    .find(|(version, _)| version == new_version)
                    .and_then(|(_, days)| *days)
                    .unwrap();
                assert_eq!(*released_at, Some(now - chrono::Duration::days(days)));
            }
        }

        // この実行で既に分かった公開日は問い合わせの回数に数えない。上限より多くの版が
        // 解決済みでも、期間を満たす版まで進める
        let adapter = DatedTagsAdapter {
            dates: vec![
                ("1.0.0", Some(90)),
                ("1.1.0", Some(30)),
                ("1.2.0", None),
                ("1.3.0", None),
                ("1.4.0", None),
                ("1.5.0", None),
                ("1.6.0", None),
            ],
            now,
            lookups: std::sync::Mutex::new(Vec::new()),
            fail: false,
        };
        for version in ["1.1.0", "1.2.0", "1.3.0", "1.4.0", "1.5.0", "1.6.0"] {
            orchestrator
                .versions
                .release_date(&adapter, "owner/cached", version)
                .await
                .unwrap();
        }
        adapter.lookups.lock().unwrap().clear();
        let judge = UpdateJudge::with_time(
            UpdateFilter::new().with_min_age(Duration::from_secs(14 * 86400)),
            now,
        );
        let result = orchestrator
            .process_one_dependency(
                swift_dependency("owner/cached"),
                &adapter,
                &judge,
                None,
                None,
            )
            .await;
        assert!(
            matches!(&result.outcome, UpdateResult::Update { new_version, .. } if new_version == "1.1.0"),
            "{:?}",
            result.outcome
        );
        assert!(adapter.lookups.lock().unwrap().is_empty());

        // 取得の失敗は公開日不明にせず、取得の失敗として報告する
        let adapter = DatedTagsAdapter {
            dates: vec![("1.0.0", Some(90)), ("1.1.0", Some(30))],
            now,
            lookups: std::sync::Mutex::new(Vec::new()),
            fail: true,
        };
        let judge = UpdateJudge::with_time(
            UpdateFilter::new().with_min_age(Duration::from_secs(14 * 86400)),
            now,
        );
        let result = orchestrator
            .process_one_dependency(
                swift_dependency("owner/failing"),
                &adapter,
                &judge,
                None,
                None,
            )
            .await;
        assert!(result.fetch_error.is_some());
        assert!(matches!(
            result.outcome,
            UpdateResult::Skip {
                reason: SkipReason::FetchFailed(_),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn young_maven_major_does_not_trigger_max_change_warning() {
        let dir = TempDir::new().unwrap();
        let orchestrator = Orchestrator::new(make_args_with_path(dir.path(), &["--java"])).unwrap();
        let now = chrono::Utc::now();
        let adapter = DatedMavenAdapter {
            calls: AtomicUsize::new(0),
            now,
        };
        let judge = UpdateJudge::with_time(
            UpdateFilter::new()
                .with_include_pinned(true)
                .with_min_age(Duration::from_secs(14 * 86400))
                .with_max_change(crate::domain::ChangeLevel::Minor),
            now,
        );
        let dep = Dependency::new(
            "example:library",
            crate::domain::VersionSpec::new(
                crate::domain::VersionSpecKind::Exact,
                "3.0.0",
                "3.0.0",
            ),
            false,
            Language::Java,
        );
        let versions = ["3.0.0", "4.0.0"].map(VersionInfo::undated).to_vec();
        let resolved = orchestrator
            .resolve_java_release_dates(&dep, &adapter, &judge, versions)
            .await
            .unwrap();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            judge.judge(&dep, &resolved),
            UpdateResult::Skip {
                reason: SkipReason::AlreadyLatest,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn current_maven_release_age_is_confirmed_only_in_verbose_output() {
        use crate::domain::{VersionSpec, VersionSpecKind};
        use chrono::TimeZone;
        let now = chrono::Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap();
        let dir = TempDir::new().unwrap();
        let dep = Dependency::new(
            "example:library",
            VersionSpec::new(VersionSpecKind::Exact, "3.0.0", "3.0.0"),
            false,
            Language::Java,
        );
        let judge = UpdateJudge::with_time(
            UpdateFilter::new()
                .with_include_pinned(true)
                .with_min_age(Duration::from_secs(7 * 86400)),
            now,
        );
        for verbose in [false, true] {
            let args = if verbose {
                vec!["--java", "--include-pinned", "--verbose"]
            } else {
                vec!["--java", "--include-pinned"]
            };
            let orchestrator = Orchestrator::new(make_args_with_path(dir.path(), &args)).unwrap();
            let adapter = DatedMavenAdapter {
                calls: AtomicUsize::new(0),
                now,
            };
            let versions = vec![
                VersionInfo::new("1.0.0", now - chrono::Duration::days(30)),
                VersionInfo::undated("3.0.0"),
            ];
            let resolved = orchestrator
                .resolve_java_release_dates(&dep, &adapter, &judge, versions)
                .await
                .unwrap();
            assert_eq!(adapter.calls.load(Ordering::SeqCst), usize::from(verbose));
            let expected = if verbose {
                SkipReason::CurrentVersionTooRecent
            } else {
                SkipReason::AlreadyLatest
            };
            let result = judge.judge(&dep, &resolved);
            assert!(
                matches!(&result, UpdateResult::Skip { reason, released_at, .. } if *reason == expected && *released_at == verbose.then_some(now - chrono::Duration::days(1))),
                "verbose={verbose}: {result:?}"
            );
            // Gradle の順序では、単一のリリースと断定できない別表記が同値になる場合がある。
            let alias = Dependency::new(
                "example:library",
                VersionSpec::new(VersionSpecKind::Exact, "3-0-0", "3-0-0"),
                false,
                Language::Java,
            );
            let resolved = orchestrator
                .resolve_java_release_dates(
                    &alias,
                    &adapter,
                    &judge,
                    vec![
                        VersionInfo::new("1.0.0", now - chrono::Duration::days(30)),
                        VersionInfo::undated("3.0.0"),
                    ],
                )
                .await
                .unwrap();
            assert_eq!(adapter.calls.load(Ordering::SeqCst), usize::from(verbose));
            assert!(matches!(
                judge.judge(&alias, &resolved),
                UpdateResult::Skip {
                    reason: SkipReason::AlreadyLatest,
                    released_at: None,
                    ..
                }
            ));
        }
    }

    #[tokio::test]
    async fn current_maven_release_date_prefers_exact_spelling_before_aliases() {
        use crate::domain::{VersionSpec, VersionSpecKind};
        use chrono::TimeZone;
        let now = chrono::Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap();
        let old_date = now - chrono::Duration::days(30);
        let current_date = now - chrono::Duration::days(1);
        let dir = TempDir::new().unwrap();
        let judge = UpdateJudge::with_time(
            UpdateFilter::new()
                .with_include_pinned(true)
                .with_min_age(Duration::from_secs(7 * 86400)),
            now,
        );
        let dep = Dependency::new(
            "example:library",
            VersionSpec::new(VersionSpecKind::Exact, "3.0.0", "3.0.0"),
            false,
            Language::Java,
        );
        for (spellings, known_exact_date) in [
            (vec!["3-0-0", "3.0.0"], None),
            (vec!["3.0.0", "3-0-0"], None),
            (vec!["3-0-0"], None),
            (vec!["3.0.0"], None),
            (vec!["3-0-0", "3.0.0"], Some(old_date)),
            (vec!["3-0-0", "3.0.0"], Some(current_date)),
        ] {
            for verbose in [false, true] {
                let args = if verbose {
                    vec!["--java", "--include-pinned", "--verbose"]
                } else {
                    vec!["--java", "--include-pinned"]
                };
                let orchestrator =
                    Orchestrator::new(make_args_with_path(dir.path(), &args)).unwrap();
                let adapter = DatedMavenAdapter {
                    calls: AtomicUsize::new(0),
                    now,
                };
                let mut versions = vec![VersionInfo::new("1.0.0", old_date)];
                versions.extend(spellings.iter().map(|spelling| {
                    VersionInfo::with_released_at(
                        *spelling,
                        if *spelling == "3.0.0" {
                            known_exact_date
                        } else {
                            None
                        },
                    )
                }));
                let resolved = orchestrator
                    .resolve_java_release_dates(&dep, &adapter, &judge, versions)
                    .await
                    .unwrap();
                let expected_date = known_exact_date
                    .or_else(|| (verbose && spellings.contains(&"3.0.0")).then_some(current_date));
                let expected_reason = if expected_date == Some(current_date) {
                    SkipReason::CurrentVersionTooRecent
                } else {
                    SkipReason::AlreadyLatest
                };
                let result = judge.judge(&dep, &resolved);
                assert!(
                    matches!(&result, UpdateResult::Skip { reason, released_at, .. } if *reason == expected_reason && *released_at == expected_date),
                    "verbose={verbose}, spellings={spellings:?}: {result:?}"
                );
                assert_eq!(
                    adapter.calls.load(Ordering::SeqCst),
                    usize::from(verbose && known_exact_date.is_none()),
                    "verbose={verbose}, spellings={spellings:?}"
                );
                if spellings.contains(&"3.0.0") {
                    assert!(
                        resolved
                            .iter()
                            .filter(|info| info.version == "3-0-0")
                            .all(|info| info.released_at.is_none())
                    );
                }
            }
        }

        // exact 以外の指定では、verbose 表示だけを理由に現在版の公開日を取得しない。
        let orchestrator =
            Orchestrator::new(make_args_with_path(dir.path(), &["--java", "--verbose"])).unwrap();
        let adapter = DatedMavenAdapter {
            calls: AtomicUsize::new(0),
            now,
        };
        for (kind, raw) in [
            (VersionSpecKind::Caret, "^3.0.0"),
            (VersionSpecKind::Range, ">=3.0.0, <4.0.0"),
        ] {
            let dep = Dependency::new(
                "example:library",
                VersionSpec::new(kind, raw, "3.0.0"),
                false,
                Language::Java,
            );
            let resolved = orchestrator
                .resolve_java_release_dates(
                    &dep,
                    &adapter,
                    &judge,
                    vec![VersionInfo::undated("3-0-0"), VersionInfo::undated("3.0.0")],
                )
                .await
                .unwrap();
            assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
            assert!(matches!(
                judge.judge(&dep, &resolved),
                UpdateResult::Skip {
                    reason: SkipReason::AlreadyLatest,
                    released_at: None,
                    ..
                }
            ));
        }
    }

    #[tokio::test]
    async fn current_release_age_skip_returns_from_osv_without_a_query() {
        use crate::domain::{VersionSpec, VersionSpecKind};
        use chrono::TimeZone;
        let now = chrono::Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap();
        let dir = TempDir::new().unwrap();
        let orchestrator = Orchestrator::new(make_args_with_path(dir.path(), &["--rust"])).unwrap();
        let dep = Dependency::new(
            "library",
            VersionSpec::new(VersionSpecKind::Caret, "^3.0.0", "3.0.0").with_prefix("^"),
            false,
            Language::Rust,
        );
        let judge = UpdateJudge::with_time(
            UpdateFilter::new().with_min_age(Duration::from_secs(7 * 86400)),
            now,
        );
        let current_date = now - chrono::Duration::days(2);
        let mut versions = vec![
            VersionInfo::new("2.0.0", now - chrono::Duration::days(30)),
            VersionInfo::new("3.0.0", current_date),
            VersionInfo::new("3.0.1", now - chrono::Duration::days(30)),
        ];
        assert!(
            matches!(judge.judge(&dep, &versions), UpdateResult::Update { new_version, .. } if new_version == "3.0.1")
        );
        // フォールバックのループでは、拒否した候補を外してから再判定する。
        versions.retain(|info| {
            compare_dependency_versions(&dep, &info.version, "3.0.1") != std::cmp::Ordering::Equal
        });
        assert!(matches!(
            judge.judge(&dep, &versions),
            UpdateResult::Skip {
                reason: SkipReason::CurrentVersionTooRecent,
                ..
            }
        ));
        let adapter = DatedMavenAdapter {
            calls: AtomicUsize::new(0),
            now,
        };
        let checker = OsvChecker::new().unwrap();
        let mut warnings = Vec::new();
        let mut context = OsvJudgeContext {
            checker: &checker,
            baseline: None,
            ecosystem: "crates.io",
            bar: None,
            warnings: &mut warnings,
        };
        let result = judge_with_osv(
            &orchestrator,
            &adapter,
            &judge,
            &dep,
            versions,
            &mut context,
        )
        .await
        .unwrap();
        assert!(
            matches!(result, UpdateResult::Skip { reason: SkipReason::CurrentVersionTooRecent, released_at: Some(date), .. } if date == current_date)
        );
        assert!(warnings.is_empty());
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
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
            let mut cache = orchestrator.versions.cache.lock().await;
            cache.insert(
                cache_key,
                vec![VersionInfo {
                    version: "4.17.21".to_string(),
                    released_at: Some(chrono::Utc::now()),
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
mod lock_age_tests;
