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
use crate::manifest::{
    ManifestInfo, ManifestWriter, PnpmSettings, WriteResult, detect_manifests, get_parser,
    has_pnpm_workspace, read_git_entries, read_registry_entries,
};
use crate::osv::{OsvCheck, OsvChecker};
use crate::progress::Progress;
use crate::registry::{
    CratesIoAdapter, GitHubTagsAdapter, GitRemote, GoProxyAdapter, HttpClient, MavenCentralAdapter,
    NpmAdapter, PackagistAdapter, PyPIAdapter, RegistryAdapter, RubyGemsAdapter,
};
use crate::tauri_sync::{TAURI_CRATE, TAURI_NPM_PACKAGES, TauriVersionSync};
use crate::update::{UpdateFilter, UpdateJudge, VersionInfo, compare_versions};
use futures::stream::{self, StreamExt};
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

/// OSV.dev API への同時並列リクエスト数の上限。
/// 1 依存に紐づく candidate version 数分のチェックを並列実行する。
const OSV_CHECK_CONCURRENCY: usize = 4;

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
        })
    }

    /// カスタムHTTPクライアントでオーケストレータを作成する (テスト用)
    pub fn with_client(args: CliArgs, client: HttpClient) -> Self {
        Self {
            args,
            client,
            general_semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
            crates_io_semaphore: Arc::new(Semaphore::new(CRATES_IO_CONCURRENCY)),
            version_cache: Arc::new(Mutex::new(HashMap::new())),
            git_remote: GitRemote::new(),
            osv_checker: None,
        }
    }

    /// 共有バージョンキャッシュを設定する (モノレポの複数ディレクトリ実行用)
    pub fn with_cache(mut self, cache: VersionCache) -> Self {
        self.version_cache = cache;
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
                    let status = run_cargo_update_precise(project_dir, name, &target).await;
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

    /// git 依存の判定を実行する
    ///
    /// - branch / DefaultBranch: リモート HEAD/ブランチ commit と現在 commit を比較し、新しければ更新
    /// - tag: リモートの全タグから最新 semver を選び、現在の tag より新しければ更新
    /// - rev: `--include-pinned` が指定された場合のみ、デフォルトブランチ HEAD へ更新
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

        // ステップ1: 全ディレクトリのマニフェストファイルを検出
        progress.spinner("Detecting manifest files...");
        let mut all_manifests = Vec::new();
        for dir in directories {
            let manifests = detect_manifests(dir);
            all_manifests.extend(manifests);
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

        // CLI引数から更新フィルタを構築
        let filter = self.build_filter();
        let judge = UpdateJudge::new(filter);

        // ステップ2: マニフェストをパースし、全依存関係を収集
        progress.spinner("Parsing manifests...");
        let mut parsed_manifests = Vec::new();

        for manifest_info in manifests {
            // 言語フィルタをチェック
            if !self.should_process_language(manifest_info.language) {
                continue;
            }

            // マニフェストをパース
            let parser = get_parser(manifest_info.language);
            let content = match std::fs::read_to_string(&manifest_info.path) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(OrchestratorError::ManifestParseError {
                        path: manifest_info.path.display().to_string(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };

            let mut dependencies = match parser.parse(&content) {
                Ok(deps) => deps,
                Err(e) => {
                    errors.push(OrchestratorError::ManifestParseError {
                        path: manifest_info.path.display().to_string(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };

            // Rust プロジェクトでは Cargo.lock から git 依存の現在コミットを補完する
            if manifest_info.language == Language::Rust {
                enrich_with_cargo_lock(&manifest_info.path, &mut dependencies);
            }

            parsed_manifests.push((manifest_info, dependencies));
        }
        progress.finish_and_clear();

        // プログレスバー用に依存関係の合計数をカウント
        let total_deps: usize = parsed_manifests.iter().map(|(_, deps)| deps.len()).sum();

        // ステップ3: 各依存関係のバージョンを取得し、更新を判定
        progress.start(total_deps as u64, "Checking dependencies");

        for (manifest_info, dependencies) in parsed_manifests {
            let mut manifest_result =
                ManifestUpdateResult::new(&manifest_info.path, manifest_info.language);
            // 複数 future から共有するため Arc に変換
            let adapter: Arc<dyn RegistryAdapter + Send + Sync> =
                Arc::from(self.get_adapter(manifest_info.language));

            // 依存数に応じて並列度を調整 (1〜4)
            let concurrency = version_check_concurrency(dependencies.len());

            // 各依存関係を並列処理する。結果は入力順で返されるため
            // 出力順は安定する (`buffered`: ordered)。
            // fetch_versions は内部でレジストリ別の Semaphore を持つため、
            // crates.io のレート制限などは従来どおり尊重される。
            let results: Vec<OnePassResult> = stream::iter(dependencies)
                .map(|dep| {
                    let adapter = Arc::clone(&adapter);
                    let judge_ref = &judge;
                    async move {
                        // 早期スキップ判定
                        if let Some(reason) = judge_ref.should_skip(&dep) {
                            return OnePassResult {
                                name: dep.name.clone(),
                                outcome: ResultOutcome::Skip(UpdateResult::skip(dep, reason)),
                                fetch_error: None,
                                osv_warnings: Vec::new(),
                            };
                        }

                        // git 依存は専用ロジック
                        if dep.is_git() {
                            let result = self.judge_git_dependency(&dep).await;
                            return OnePassResult {
                                name: dep.name.clone(),
                                outcome: ResultOutcome::Skip(result),
                                fetch_error: None,
                                osv_warnings: Vec::new(),
                            };
                        }

                        // registry 経由のフェッチ
                        match self.fetch_versions(&*adapter, &dep.name).await {
                            Ok(versions) => {
                                // OSV チェック: 脆弱な candidate を除外する。
                                // Swift など osv_ecosystem() == None の言語はスキップ。
                                let (versions, osv_warnings) =
                                    match (self.osv_checker.as_ref(), dep.language.osv_ecosystem())
                                    {
                                        (Some(checker), Some(eco)) => {
                                            filter_vulnerable_versions(
                                                checker, &dep.name, eco, versions,
                                            )
                                            .await
                                        }
                                        _ => (versions, Vec::new()),
                                    };
                                let result = judge_ref.judge(&dep, &versions);
                                OnePassResult {
                                    name: dep.name.clone(),
                                    outcome: ResultOutcome::Skip(result),
                                    fetch_error: None,
                                    osv_warnings,
                                }
                            }
                            Err(e) => {
                                let err_msg = e.clone();
                                OnePassResult {
                                    name: dep.name.clone(),
                                    outcome: ResultOutcome::Skip(UpdateResult::skip(
                                        dep,
                                        SkipReason::FetchFailed(e),
                                    )),
                                    fetch_error: Some(err_msg),
                                    osv_warnings: Vec::new(),
                                }
                            }
                        }
                    }
                })
                .buffered(concurrency)
                .collect()
                .await;

            // progress / errors / manifest_result を順序を保って反映する
            for result in results {
                progress.set_message(&format!("Checking {}", &result.name));
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
                match result.outcome {
                    ResultOutcome::Skip(r) => manifest_result.add_result(r),
                }
                progress.inc();
            }

            summary.add_manifest(manifest_result);
        }
        progress.finish_and_clear();

        // ステップ3.5: Tauriプロジェクトの場合、バージョンを同期
        let is_tauri = manifests.iter().any(|m| m.is_tauri_rust);
        if is_tauri {
            progress.spinner("Synchronizing Tauri versions...");
            self.synchronize_tauri_versions(&mut summary, &mut errors)
                .await;
            progress.finish_and_clear();
        }

        // ステップ4: 更新を適用 (ドライランでなければ)
        if !self.args.dry_run {
            progress.spinner("Writing updates...");
        }
        let writer = ManifestWriter::new(self.args.dry_run);
        let write_results = writer.apply_all_updates(&summary.manifests, get_parser);
        progress.finish_and_clear();

        // 書き込みエラーを収集
        for result in &write_results {
            for error in &result.errors {
                errors.push(OrchestratorError::WriteError {
                    path: result.path.display().to_string(),
                    message: error.clone(),
                });
            }
        }

        OrchestratorResult {
            summary,
            write_results,
            errors,
        }
    }

    /// CLI引数からUpdateFilterを構築する
    fn build_filter(&self) -> UpdateFilter {
        let mut filter = UpdateFilter::new();

        // 言語フィルタ
        if self.args.has_language_filter() {
            let mut languages = Vec::new();
            if self.args.node {
                languages.push(Language::Node);
            }
            if self.args.python {
                languages.push(Language::Python);
            }
            if self.args.rust_lang {
                languages.push(Language::Rust);
            }
            if self.args.go {
                languages.push(Language::Go);
            }
            if self.args.ruby {
                languages.push(Language::Ruby);
            }
            if self.args.php {
                languages.push(Language::Php);
            }
            if self.args.java {
                languages.push(Language::Java);
            }
            if self.args.swift {
                languages.push(Language::Swift);
            }
            filter = filter.with_languages(languages);
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

        // 経過日数フィルタ
        // 優先順位: CLI --age > pnpm設定 (Node.jsプロジェクトの場合)
        if let Some(age) = self.args.age {
            filter = filter.with_min_age(age);
        } else if has_pnpm_workspace(&self.args.path) {
            // pnpm設定から最小リリース経過日数を読み取る
            let pnpm_settings = PnpmSettings::from_dir(&self.args.path);
            if let Some(age) = pnpm_settings.minimum_release_age {
                filter = filter.with_min_age(age);
            }
        }

        filter
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

        // 全Tauri npmパッケージにnpmバージョンの調整を適用
        if let Some(ref target) = npm_target_version {
            for (manifest_idx, result_idx, original, _current) in &npm_packages {
                match original {
                    UpdateResult::Update { dependency, .. } => {
                        // 既存の更新を調整
                        let adjusted = UpdateResult::update(dependency.clone(), target);
                        summary.manifests[*manifest_idx].results[*result_idx] = adjusted;
                    }
                    UpdateResult::Skip { dependency, .. } => {
                        // スキップから新しい更新を作成
                        let adjusted = UpdateResult::update(dependency.clone(), target);
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
    outcome: ResultOutcome,
    /// fetch 失敗時の原因メッセージ (存在する場合のみ `OrchestratorError` として記録される)
    fetch_error: Option<String>,
    /// OSV チェックで除外・問題が生じた version のメッセージ
    osv_warnings: Vec<String>,
}

enum ResultOutcome {
    Skip(UpdateResult),
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

/// OSV.dev API を使い、各 candidate version を脆弱性チェックする。
///
/// 戻り値: `(脆弱性なし or 判定不能なバージョン一覧, 警告メッセージ一覧)`
/// - 脆弱性が確認されたバージョンは安全リストから除外され、警告に含める
/// - API エラー等で判定できなかったバージョンは「安全側」として残し、警告に含める
async fn filter_vulnerable_versions(
    checker: &OsvChecker,
    name: &str,
    ecosystem: &str,
    versions: Vec<VersionInfo>,
) -> (Vec<VersionInfo>, Vec<String>) {
    let checks = versions.iter().map(|v| {
        let checker = checker.clone();
        let name = name.to_string();
        let ecosystem = ecosystem.to_string();
        let version = v.version.clone();
        async move { checker.check(&ecosystem, &name, &version).await }
    });

    let outcomes: Vec<Result<OsvCheck, String>> = stream::iter(checks)
        .buffered(OSV_CHECK_CONCURRENCY)
        .collect()
        .await;

    let mut safe = Vec::with_capacity(versions.len());
    let mut warnings = Vec::new();
    for (v, outcome) in versions.into_iter().zip(outcomes) {
        match outcome {
            Ok(OsvCheck::Safe) => safe.push(v),
            Ok(OsvCheck::Vulnerable(ids)) => {
                let detail = if ids.is_empty() {
                    "no advisory IDs".to_string()
                } else {
                    ids.join(", ")
                };
                warnings.push(format!("{} vulnerable, skipped ({})", v.version, detail));
            }
            Err(e) => {
                warnings.push(format!("{} could not be checked: {}", v.version, e));
                safe.push(v);
            }
        }
    }

    (safe, warnings)
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

/// `cargo update -p <name> --precise <version>` を実行する。
/// resolver 制約違反など失敗ケースでは stderr を保持した `UpdateCommandFailed` を返す。
///
/// `tokio::process::Command` を使い、`cargo update` の長時間実行で tokio エグゼキュータの
/// ワーカースレッドがブロックされて他の async タスク (HTTP リクエスト等) が止まるのを防ぐ。
async fn run_cargo_update_precise(project_dir: &Path, name: &str, version: &str) -> LockAgeStatus {
    match Command::new("cargo")
        .args(["update", "-p", name, "--precise", version])
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

/// Cargo.toml と同じディレクトリの Cargo.lock を読み込み、
/// git 依存の `current_commit` をセットする。
fn enrich_with_cargo_lock(cargo_toml_path: &Path, dependencies: &mut [Dependency]) {
    let Some(dir) = cargo_toml_path.parent() else {
        return;
    };
    let lock_path = dir.join("Cargo.lock");
    let git_entries = read_git_entries(&lock_path);
    if git_entries.is_empty() {
        return;
    }
    for dep in dependencies.iter_mut() {
        if let Some(git) = dep.git_source.as_mut()
            && let Some(entry) = git_entries.get(&dep.name)
        {
            git.current_commit = Some(entry.commit.clone());
        }
    }
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

/// 指定されたタグ群から最新の semver 互換タグを選ぶ。
/// プレリリースは除外する。
fn latest_semver_tag(tags: &[String]) -> Option<String> {
    let stable: Vec<&String> = tags
        .iter()
        .filter(|t| !crate::update::is_prerelease_version(t))
        .filter(|t| {
            // 少なくとも 1 つは数字を含むバージョンらしい文字列のみ許容
            t.chars().any(|c| c.is_ascii_digit())
        })
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
    fn test_build_filter_cli_age_overrides_pnpm() {
        let dir = TempDir::new().unwrap();

        // minimumReleaseAge 付きの pnpm-workspace.yaml を作成
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();

        // CLI --age が pnpm 設定を上書きすべき
        let args = make_args_with_path(dir.path(), &["--age", "2w"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        // CLI の age (2週間) であるべきで、pnpm の age (10日) ではない
        assert!(filter.min_age.is_some());
        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(14 * 24 * 60 * 60) // 2週間
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
    fn test_build_filter_no_pnpm_no_age() {
        let dir = TempDir::new().unwrap();

        // pnpmファイルなし、--ageフラグなし
        let args = make_args_with_path(dir.path(), &[]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        // min_ageは設定されないべき
        assert!(filter.min_age.is_none());
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
