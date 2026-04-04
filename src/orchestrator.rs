//! 更新オーケストレータ - 更新ワークフロー全体の調整
//!
//! このモジュールは以下を提供する:
//! - ワークフロー調整: 検出 → パース → フェッチ → 判定 → 書き込み
//! - レート制限付き並列レジストリクエリ
//! - ドライランモード対応
//! - 言語・パッケージフィルタの適用
//! - 部分的な継続を伴うエラーハンドリング

use crate::cli::CliArgs;
use crate::domain::{Language, ManifestUpdateResult, SkipReason, UpdateResult, UpdateSummary};
use crate::manifest::{
    ManifestInfo, ManifestWriter, PnpmSettings, WriteResult, detect_manifests, get_parser,
    has_pnpm_workspace,
};
use crate::progress::Progress;
use crate::registry::{
    CratesIoAdapter, GitHubTagsAdapter, GoProxyAdapter, HttpClient, MavenCentralAdapter,
    NpmAdapter, PackagistAdapter, PyPIAdapter, RegistryAdapter, RubyGemsAdapter,
};
use crate::tauri_sync::{TAURI_CRATE, TAURI_NPM_PACKAGES, TauriVersionSync};
use crate::update::{UpdateFilter, UpdateJudge, VersionInfo};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

/// レジストリリクエストのデフォルト同時実行数
const DEFAULT_CONCURRENCY: usize = 10;

/// crates.io 用の同時実行数 (レート制限あり)
const CRATES_IO_CONCURRENCY: usize = 1;

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
        }
    }
}

impl std::error::Error for OrchestratorError {}

impl Orchestrator {
    /// 指定されたCLI引数で新しいオーケストレータを作成する
    pub fn new(args: CliArgs) -> Result<Self, OrchestratorError> {
        let client =
            HttpClient::new().map_err(|e| OrchestratorError::HttpClientError(e.to_string()))?;

        Ok(Self {
            args,
            client,
            general_semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
            crates_io_semaphore: Arc::new(Semaphore::new(CRATES_IO_CONCURRENCY)),
            version_cache: Arc::new(Mutex::new(HashMap::new())),
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
        }
    }

    /// 共有バージョンキャッシュを設定する (モノレポの複数ディレクトリ実行用)
    pub fn with_cache(mut self, cache: VersionCache) -> Self {
        self.version_cache = cache;
        self
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

            let dependencies = match parser.parse(&content) {
                Ok(deps) => deps,
                Err(e) => {
                    errors.push(OrchestratorError::ManifestParseError {
                        path: manifest_info.path.display().to_string(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };

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
            let adapter = self.get_adapter(manifest_info.language);

            for dep in dependencies {
                progress.set_message(&format!("Checking {}", &dep.name));

                // この依存関係を早期スキップすべきかチェック
                if let Some(reason) = judge.should_skip(&dep) {
                    manifest_result.add_result(UpdateResult::skip(dep, reason));
                    progress.inc();
                    continue;
                }

                // レジストリからバージョンを取得
                let versions = match self.fetch_versions(&*adapter, &dep.name).await {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(OrchestratorError::RegistryError {
                            package: dep.name.clone(),
                            message: e.to_string(),
                        });
                        manifest_result
                            .add_result(UpdateResult::skip(dep, SkipReason::FetchFailed(e)));
                        progress.inc();
                        continue;
                    }
                };

                // 更新すべきか判定
                let result = judge.judge(&dep, &versions);
                manifest_result.add_result(result);
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
