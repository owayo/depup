//! 更新オーケストレータ - 更新ワークフロー全体の調整
//!
//! このモジュールは以下を提供する:
//! - ワークフロー調整: 検出 → パース → フェッチ → 判定 → 書き込み
//! - レート制限付き並列レジストリクエリ
//! - ドライランモード対応
//! - 言語・パッケージフィルタの適用
//! - 部分的な継続を伴うエラーハンドリング

use crate::cli::CliArgs;
use crate::domain::{
    Dependency, GitReference, Language, ManifestUpdateResult, SkipReason, UpdateResult,
    UpdateSummary,
};
use crate::global_config::{DEFAULT_AGE, GlobalConfig};
use crate::manifest::{
    BunSettings, ManifestInfo, ManifestWriter, PnpmSettings, WriteResult, detect_manifests,
    find_cargo_lock_upward, get_parser, has_bunfig, has_pnpm_workspace, read_git_entries,
    read_registry_entries,
};
use crate::osv::{OsvCheck, OsvChecker};
use crate::progress::Progress;
use crate::registry::{
    CratesIoAdapter, GitHubTagsAdapter, GitRemote, GoProxyAdapter, HttpClient, MavenCentralAdapter,
    NpmAdapter, PackagistAdapter, PyPIAdapter, RegistryAdapter, RubyGemsAdapter,
};
use crate::tauri_sync::{TAURI_CRATE, TAURI_NPM_PACKAGES, TauriVersionSync};
use crate::update::{
    UpdateFilter, UpdateJudge, VersionInfo, compare_dependency_versions, compare_versions,
};
use futures::stream::{self, StreamExt};
use indicatif::ProgressBar;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
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

/// バージョン情報のキャッシュ (言語, パッケージ名) をキーとする
pub type VersionCache = Arc<Mutex<HashMap<(Language, String), Vec<VersionInfo>>>>;

/// プロジェクト直下から検出した minimumReleaseAge とそのソース
struct ProjectAge {
    duration: Duration,
    source: String,
}

/// パース済みマニフェスト (parse_phase → check_phase の受け渡し用)
struct ParsedManifest<'a> {
    info: &'a ManifestInfo,
    dependencies: Vec<Dependency>,
}

/// `resolve_age_policy` が返す age 解決結果。
/// `notice` の表示は呼び出し側で行う (副作用を解決ロジックから分離)。
struct ResolvedAge {
    duration: Option<Duration>,
    notice: Option<AgeNotice>,
}

/// age 解決の過程でユーザーに伝えるべき通知。
enum AgeNotice {
    /// CLI 指定 (`--age` / `--no-age`) がプロジェクト minimumReleaseAge に上書きされた (警告)
    CliOverriddenByProject {
        cli_label: &'static str,
        days: u64,
        source: String,
    },
    /// CLI 指定なし、プロジェクト minimumReleaseAge を採用 (情報通知)
    UsingProjectPolicy { days: u64, source: String },
}

/// age 制約を優先順位どおりに解決する (純粋関数)。
///
/// 優先順位:
///   1. minimumReleaseAge (pnpm-workspace.yaml / bunfig.toml) — プロジェクトポリシー強制
///   2. CLI `--age`
///   3. CLI `--no-age`
///   4. グローバル設定 (~/.config/depup/config.toml)
///   5. 組み込みデフォルト (`DEFAULT_AGE`)
fn resolve_age_policy(
    project_age: Option<&ProjectAge>,
    cli_age: Option<Duration>,
    cli_no_age: bool,
    config_age: Option<Duration>,
) -> ResolvedAge {
    if let Some(project) = project_age {
        let days = project.duration.as_secs() / 86400;
        let notice = if cli_age.is_some() || cli_no_age {
            let cli_label = if cli_no_age { "--no-age" } else { "--age" };
            AgeNotice::CliOverriddenByProject {
                cli_label,
                days,
                source: project.source.clone(),
            }
        } else {
            AgeNotice::UsingProjectPolicy {
                days,
                source: project.source.clone(),
            }
        };
        return ResolvedAge {
            duration: Some(project.duration),
            notice: Some(notice),
        };
    }

    let duration = if cli_no_age {
        None
    } else if let Some(age) = cli_age {
        Some(age)
    } else if let Some(cfg_age) = config_age {
        Some(cfg_age)
    } else {
        Some(DEFAULT_AGE)
    };
    ResolvedAge {
        duration,
        notice: None,
    }
}

/// `AgeNotice` を stderr に表示する。
fn emit_age_notice(notice: &AgeNotice) {
    use colored::Colorize as _;
    fn unit(days: u64) -> &'static str {
        if days == 1 { "day" } else { "days" }
    }
    match notice {
        AgeNotice::CliOverriddenByProject {
            cli_label,
            days,
            source,
        } => {
            let msg = format!(
                "⚠ {} ignored: project's minimumReleaseAge ({} {} from {}) takes precedence",
                cli_label,
                days,
                unit(*days),
                source,
            );
            eprintln!("{}", msg.yellow());
        }
        AgeNotice::UsingProjectPolicy { days, source } => {
            let msg = format!(
                "ℹ Using project's minimumReleaseAge ({} {} from {})",
                days,
                unit(*days),
                source,
            );
            eprintln!("{}", msg.cyan());
        }
    }
}

/// 更新ワークフローを調整するオーケストレータ
pub struct Orchestrator {
    /// 設定用CLI引数
    args: CliArgs,
    /// レジストリリクエスト用HTTPクライアント
    client: HttpClient,
    /// 汎用同時実行制御用セマフォ
    general_semaphore: Arc<Semaphore>,
    /// crates.io 専用レート制限セマフォ
    crates_io_semaphore: Arc<Semaphore>,
    /// ディレクトリ間で共有されるバージョンキャッシュ
    version_cache: VersionCache,
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
            client,
            general_semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
            crates_io_semaphore: Arc::new(Semaphore::new(CRATES_IO_CONCURRENCY)),
            version_cache: Arc::new(Mutex::new(HashMap::new())),
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

    /// Rust プロジェクトの Cargo.lock を走査し、`--age` を満たさない
    /// transitive 依存を検出して `cargo update -p --precise <older_version>` で差し戻す。
    ///
    /// 典型的な用途: `depup --age 2w --install` 実行時、`cargo update` が
    /// semver 解決で 2 週間以内にリリースされた transitive 依存を引き込んでしまう場合に
    /// それらを age 境界以前の最新バージョンへ戻す。
    ///
    /// 1 つの依存を差し戻すと cargo が依存サブツリーを再解決し、別の依存が
    /// 新たに age 違反になる可能性があるため、最大 `MAX_ENFORCE_LOCK_AGE_PASSES` 回
    /// 反復する。変化がなくなった時点で終了する。
    ///
    /// 戻り値: 反復全体で試行された調整のリスト (最終結果の集約)
    pub async fn enforce_lock_age_rust(
        &self,
        project_dir: &Path,
        min_age: Duration,
    ) -> Vec<LockAgeAdjustment> {
        let Ok(chrono_duration) = chrono::Duration::from_std(min_age) else {
            return Vec::new();
        };
        let cutoff = chrono::Utc::now() - chrono_duration;
        let adapter = CratesIoAdapter::new(self.client.clone());

        let mut aggregated: Vec<LockAgeAdjustment> = Vec::new();
        let mut previously_tried: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();

        for _pass in 0..MAX_ENFORCE_LOCK_AGE_PASSES {
            let lock_path = project_dir.join("Cargo.lock");
            let entries = read_registry_entries(&lock_path);
            if entries.is_empty() {
                break;
            }

            let mut pass_adjustments: Vec<LockAgeAdjustment> = Vec::new();
            let mut any_downgraded = false;

            for (name, versions) in &entries {
                let all_versions = match self.fetch_versions(&adapter, name).await {
                    Ok(v) => v,
                    Err(_) => {
                        for v in versions {
                            let key = (name.clone(), v.clone());
                            if previously_tried.insert(key) {
                                pass_adjustments.push(LockAgeAdjustment {
                                    name: name.clone(),
                                    from: v.clone(),
                                    to: None,
                                    status: LockAgeStatus::ReleaseDateUnavailable,
                                });
                            }
                        }
                        continue;
                    }
                };

                for current in versions {
                    let key = (name.clone(), current.clone());
                    if previously_tried.contains(&key) {
                        // 既に試行済みの (name, version) 組合せはスキップ (無限ループ回避)
                        continue;
                    }

                    let Some(current_info) = all_versions.iter().find(|v| {
                        compare_versions(&v.version, current) == std::cmp::Ordering::Equal
                    }) else {
                        continue;
                    };

                    if current_info.released_at <= cutoff {
                        continue;
                    }

                    let Some(target) = pick_older_within_age(&all_versions, current, cutoff) else {
                        previously_tried.insert(key.clone());
                        pass_adjustments.push(LockAgeAdjustment {
                            name: name.clone(),
                            from: current.clone(),
                            to: None,
                            status: LockAgeStatus::NoOlderCandidate,
                        });
                        continue;
                    };

                    previously_tried.insert(key.clone());
                    let status =
                        run_cargo_update_precise(project_dir, name, current, &target).await;
                    let (adjust_to, downgraded) = match &status {
                        LockAgeStatus::Downgraded => (Some(target.clone()), true),
                        _ => (None, false),
                    };
                    if downgraded {
                        any_downgraded = true;
                    }
                    pass_adjustments.push(LockAgeAdjustment {
                        name: name.clone(),
                        from: current.clone(),
                        to: adjust_to,
                        status,
                    });
                }
            }

            aggregated.extend(pass_adjustments);

            if !any_downgraded {
                // このパスで実際の差し戻しが発生しなかった → 収束
                break;
            }
        }

        aggregated
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

        let judge = UpdateJudge::new(self.build_filter());

        let parsed = self.parse_phase(manifests, progress, &mut errors);
        self.check_phase(parsed, &judge, progress, &mut summary, &mut errors)
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
                enrich_with_cargo_lock(&info.path, &self.args.path, &mut dependencies);
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
        judge: &UpdateJudge,
        progress: &mut Progress,
        summary: &mut UpdateSummary,
        errors: &mut Vec<OrchestratorError>,
    ) {
        let total_deps: usize = parsed.iter().map(|p| p.dependencies.len()).sum();
        progress.start(total_deps as u64, "Checking dependencies");
        let progress_bar = progress.bar();

        for ParsedManifest { info, dependencies } in parsed {
            let mut manifest_result = ManifestUpdateResult::new(&info.path, info.language);
            // 複数 future から共有するため Arc に変換
            let adapter: Arc<dyn RegistryAdapter + Send + Sync> =
                Arc::from(self.get_adapter(info.language));

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
        self.synchronize_tauri_versions(summary, errors).await;
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
    fn build_filter(&self) -> UpdateFilter {
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

        // 経過日数フィルタ (解決ロジックは `resolve_age` / `resolve_age_policy` に分離)
        let resolved = self.resolve_age();
        if let Some(notice) = resolved.notice.as_ref() {
            emit_age_notice(notice);
        }
        if let Some(age) = resolved.duration {
            filter = filter.with_min_age(age);
        }

        // 変更レベル上限
        if let Some(level) = self.args.max_change {
            filter = filter.with_max_change(level);
        }

        filter
    }

    /// age 制約を優先順位どおりに解決する。
    /// judge (`build_filter`) と install フェーズ (`resolved_min_age`) で同じ解決ロジックを
    /// 共有し、direct deps と transitive deps の age ポリシーを揃える。
    fn resolve_age(&self) -> ResolvedAge {
        let project_age = self.read_project_minimum_release_age();
        resolve_age_policy(
            project_age.as_ref(),
            self.args.age,
            self.args.no_age,
            self.global_config.as_ref().and_then(|c| c.age_duration()),
        )
    }

    /// install フェーズ (PM install / Rust lock audit) に適用する解決済み age を返す。
    /// judge と同じ優先順位で解決するため、CLI `--age` 未指定でもプロジェクト
    /// minimumReleaseAge / グローバル設定 / 組み込みデフォルト (1w) が transitive 依存へ反映される。
    /// notice は `build_filter` で既に発行済みのため、ここでは再発行しない。
    pub fn resolved_min_age(&self) -> Option<Duration> {
        self.resolve_age().duration
    }

    /// プロジェクト直下の minimumReleaseAge 設定を読む。
    /// pnpm (`pnpm-workspace.yaml` / `.npmrc` / `package.json`) と
    /// bun (`bunfig.toml`) の両方を見て、両方ある場合はより厳しい方 (max) を採用する。
    fn read_project_minimum_release_age(&self) -> Option<ProjectAge> {
        let mut candidates: Vec<(Duration, &'static str)> = Vec::new();

        if has_pnpm_workspace(&self.args.path)
            && let Some((age, source)) =
                PnpmSettings::minimum_release_age_with_source(&self.args.path)
        {
            // source は実際に値が読まれたファイル (.npmrc / pnpm-workspace.yaml / package.json)
            candidates.push((age, source));
        }
        if has_bunfig(&self.args.path) {
            let bun = BunSettings::from_dir(&self.args.path);
            if let Some(age) = bun.minimum_release_age {
                candidates.push((age, "bunfig.toml"));
            }
        }

        candidates
            .into_iter()
            .max_by_key(|(d, _)| *d)
            .map(|(duration, source)| ProjectAge {
                duration,
                source: source.to_string(),
            })
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
        }
    }

    /// 言語に対応するレジストリアダプタを取得する
    fn get_adapter(&self, language: Language) -> Box<dyn RegistryAdapter + Send + Sync> {
        match language {
            Language::Node => Box::new(NpmAdapter::new(self.client.clone())),
            Language::Python => Box::new(PyPIAdapter::new(self.client.clone())),
            Language::Rust => Box::new(CratesIoAdapter::new(self.client.clone())),
            Language::Go => Box::new(GoProxyAdapter::new(self.client.clone())),
            Language::Ruby => Box::new(RubyGemsAdapter::new(self.client.clone())),
            Language::Php => Box::new(PackagistAdapter::new(self.client.clone())),
            Language::Java => Box::new(MavenCentralAdapter::new(self.client.clone())),
            Language::Swift => Box::new(GitHubTagsAdapter::new(self.client.clone())),
        }
    }

    /// 同時実行制御とキャッシュ付きでレジストリからバージョンを取得する
    async fn fetch_versions(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> Result<Vec<VersionInfo>, String> {
        let cache_key = (adapter.language(), package.to_string());

        // まずキャッシュを確認
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

        // セマフォ取得後にキャッシュを再確認（同一パッケージの並行フェッチを防止）
        {
            let cache = self.version_cache.lock().await;
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

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
    ) {
        use crate::tauri_sync::extract_major_minor;

        // Nodeマニフェスト内の全Tauri npmパッケージを検索
        // 戻り値: Vec<(manifest_idx, result_idx, result, current_version)>
        let npm_packages: Vec<(usize, usize, UpdateResult, String)> = summary
            .manifests
            .iter()
            .enumerate()
            .filter(|(_, m)| m.language == Language::Node)
            .flat_map(|(mi, m)| {
                m.results
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| TAURI_NPM_PACKAGES.contains(&r.package_name()))
                    .map(move |(ri, r)| {
                        let current = r.dependency().version().to_string();
                        (mi, ri, r.clone(), current)
                    })
            })
            .collect();

        // Rustマニフェスト内のtauri crateを検索
        let crate_info: Option<(usize, usize, UpdateResult, String)> = summary
            .manifests
            .iter()
            .enumerate()
            .filter(|(_, m)| m.language == Language::Rust)
            .flat_map(|(mi, m)| {
                m.results
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.package_name() == TAURI_CRATE)
                    .map(move |(ri, r)| {
                        let current = r.dependency().version().to_string();
                        (mi, ri, r.clone(), current)
                    })
            })
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

        // 保留中の更新後の実効バージョンを決定
        let npm_effective = npm_packages.first().map(|(_, _, r, current)| match r {
            UpdateResult::Update { new_version, .. } => new_version.as_str(),
            _ => current.as_str(),
        });

        let crate_effective = crate_info.as_ref().map(|(_, _, r, current)| match r {
            UpdateResult::Update { new_version, .. } => new_version.as_str(),
            _ => current.as_str(),
        });

        // バージョンが既に一致しているか確認 - 一致していれば同期不要
        if let (Some(npm_v), Some(crate_v)) = (npm_effective, crate_effective)
            && let (Some(npm_mm), Some(crate_mm)) =
                (extract_major_minor(npm_v), extract_major_minor(crate_v))
            && npm_mm == crate_mm
        {
            return;
        }

        // バージョンが不一致 - 同期が必要

        // 両方のレジストリからバージョンを取得
        let npm_adapter = self.get_adapter(Language::Node);
        let crate_adapter = self.get_adapter(Language::Rust);

        // バージョン取得には最初のnpmパッケージ名を使用 (全パッケージでバージョンは共通)
        let npm_pkg_name = TAURI_NPM_PACKAGES[0];
        let npm_versions = match self.fetch_versions(&*npm_adapter, npm_pkg_name).await {
            Ok(v) => v,
            Err(e) => {
                errors.push(OrchestratorError::RegistryError {
                    package: npm_pkg_name.to_string(),
                    message: format!("Failed to fetch for Tauri sync: {}", e),
                });
                return;
            }
        };

        let crate_versions = match self.fetch_versions(&*crate_adapter, TAURI_CRATE).await {
            Ok(v) => v,
            Err(e) => {
                errors.push(OrchestratorError::RegistryError {
                    package: TAURI_CRATE.to_string(),
                    message: format!("Failed to fetch for Tauri sync: {}", e),
                });
                return;
            }
        };

        // 同期先の候補にも judge と同じ解決済み age を適用する
        // (同期が age ポリシーを迂回して新しすぎるバージョンを書かないように)
        let cutoff = self.resolved_min_age().and_then(|age| {
            chrono::Duration::from_std(age)
                .ok()
                .map(|d| chrono::Utc::now() - d)
        });
        let npm_versions = filter_versions_by_cutoff(npm_versions, cutoff);
        let crate_versions = filter_versions_by_cutoff(crate_versions, cutoff);

        // 同期ヘルパーを作成し、同期後のバージョンを取得
        let sync = TauriVersionSync::new(npm_versions, crate_versions);

        let npm_update_result = npm_packages.first().map(|(_, _, r, _)| r);
        let crate_update_result = crate_info.as_ref().map(|(_, _, r, _)| r);

        let (npm_target_version, crate_target_version) = sync.synchronize_with_current(
            npm_current,
            npm_update_result,
            crate_info.as_ref().map(|(_, _, _, v)| v.as_str()),
            crate_update_result,
        );

        // 全Tauri npmパッケージにnpmバージョンの調整を適用。
        // @tauri-apps/api と @tauri-apps/cli はパッチバージョン集合が一致しない
        // ことがあるため、パッケージごとに実在するバージョンを選ぶ。
        if let Some(ref target) = npm_target_version {
            for (manifest_idx, result_idx, original, _current) in &npm_packages {
                let pkg_name = original.package_name();
                let pkg_target = if pkg_name == npm_pkg_name {
                    Some(target.clone())
                } else {
                    match self.fetch_versions(&*npm_adapter, pkg_name).await {
                        Ok(vs) => {
                            let vs = filter_versions_by_cutoff(vs, cutoff);
                            pick_sync_version(&vs, target)
                        }
                        // フェッチできない場合はこのパッケージの同期を見送る
                        Err(_) => None,
                    }
                };
                let Some(pkg_target) = pkg_target else {
                    continue;
                };
                match original {
                    UpdateResult::Update { dependency, .. } => {
                        // 既存の更新を調整
                        let adjusted = UpdateResult::update(dependency.clone(), pkg_target);
                        summary.manifests[*manifest_idx].results[*result_idx] = adjusted;
                    }
                    UpdateResult::Skip { dependency, .. } => {
                        // スキップから新しい更新を作成
                        let adjusted = UpdateResult::update(dependency.clone(), pkg_target);
                        summary.manifests[*manifest_idx].results[*result_idx] = adjusted;
                        summary.manifests[*manifest_idx].modified = true;
                    }
                }
            }
        }

        // crateバージョンの調整を適用
        if let Some(ref target) = crate_target_version
            && let Some((manifest_idx, result_idx, original, _)) = crate_info
        {
            match original {
                UpdateResult::Update { dependency, .. } => {
                    let adjusted = UpdateResult::update(dependency, target);
                    summary.manifests[manifest_idx].results[result_idx] = adjusted;
                }
                UpdateResult::Skip { dependency, .. } => {
                    let adjusted = UpdateResult::update(dependency, target);
                    summary.manifests[manifest_idx].results[result_idx] = adjusted;
                    summary.manifests[manifest_idx].modified = true;
                }
            }
        }
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

/// `enforce_lock_age_rust` が 1 件の依存に対して実施した調整内容
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockAgeAdjustment {
    /// 対象パッケージ名
    pub name: String,
    /// Cargo.lock 上で解決されていたバージョン
    pub from: String,
    /// `cargo update -p --precise` で差し戻したバージョン (Ok のみ)
    pub to: Option<String>,
    /// `cargo update -p --precise` の実行結果 (Ok / Err の stderr)
    pub status: LockAgeStatus,
}

/// `enforce_lock_age_rust` の 1 件分ステータス
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockAgeStatus {
    /// 違反バージョンを `cargo update -p --precise` で差し戻した
    Downgraded,
    /// age 内の代替バージョンが見つからずスキップ (全バージョンが新しい等)
    NoOlderCandidate,
    /// `cargo update -p --precise` がエラーを返した (resolver 制約違反など)
    UpdateCommandFailed(String),
    /// レジストリからの release 日取得に失敗
    ReleaseDateUnavailable,
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
) -> Option<String> {
    let mut best: Option<&VersionInfo> = None;
    for v in available {
        if v.is_prerelease() {
            continue;
        }
        if v.released_at > cutoff {
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
/// `tokio::process::Command` を使い、`cargo update` の長時間実行で tokio エグゼキュータの
/// ワーカースレッドがブロックされて他の async タスク (HTTP リクエスト等) が止まるのを防ぐ。
async fn run_cargo_update_precise(
    project_dir: &Path,
    name: &str,
    current: &str,
    version: &str,
) -> LockAgeStatus {
    let spec = format!("{name}@{current}");
    match Command::new("cargo")
        .args(["update", "-p", &spec, "--precise", version])
        .current_dir(project_dir)
        .output()
        .await
    {
        Ok(output) if output.status.success() => LockAgeStatus::Downgraded,
        Ok(output) => LockAgeStatus::UpdateCommandFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ),
        Err(e) => LockAgeStatus::UpdateCommandFailed(e.to_string()),
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
fn tauri_sync_protected(result: &UpdateResult) -> bool {
    match result {
        UpdateResult::Update { .. } => false,
        UpdateResult::Skip { reason, .. } => !matches!(
            reason,
            SkipReason::AlreadyLatest | SkipReason::NoSuitableVersion
        ),
    }
}

/// cutoff (現在時刻 - age) より新しいバージョンを候補から除外する
fn filter_versions_by_cutoff(
    versions: Vec<VersionInfo>,
    cutoff: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<VersionInfo> {
    match cutoff {
        Some(c) => versions
            .into_iter()
            .filter(|v| v.released_at <= c)
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
            pick_older_within_age(&versions, "1.5.0", cutoff).as_deref(),
            Some("1.4.9"),
        );
    }

    #[test]
    fn test_pick_older_within_age_returns_none_when_all_newer() {
        // 全候補が age 違反または現バージョン以上
        let versions = vec![version_at("1.5.0", 3), version_at("1.6.0", 1)];
        let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
        assert!(pick_older_within_age(&versions, "1.5.0", cutoff).is_none());
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
            pick_older_within_age(&versions, "1.5.0", cutoff).as_deref(),
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
            pick_older_within_age(&versions, "2.0.0", cutoff).as_deref(),
            Some("1.4.9"),
        );
    }

    #[test]
    fn test_pick_older_within_age_ignores_same_version() {
        // current と同じバージョンは候補外 (downgrade できない)
        let versions = vec![version_at("1.4.0", 30), version_at("1.5.0", 3)];
        let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
        assert_eq!(
            pick_older_within_age(&versions, "1.5.0", cutoff).as_deref(),
            Some("1.4.0"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::fs;
    use tempfile::TempDir;

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
