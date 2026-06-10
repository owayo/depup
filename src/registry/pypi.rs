//! PyPI JSON API アダプタ
//!
//! PyPI からパッケージバージョン情報を取得する。
//! API エンドポイント: https://pypi.org/pypi/{package}/json

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

/// PyPI API のベース URL
const PYPI_API_URL: &str = "https://pypi.org/pypi";

/// PyPI アダプタ
pub struct PyPIAdapter {
    client: HttpClient,
}

/// PyPI パッケージメタデータレスポンス
#[derive(Debug, Deserialize)]
struct PyPIResponse {
    /// バージョンごとのリリース情報
    releases: HashMap<String, Vec<ReleaseInfo>>,
}

/// リリースファイル情報
#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    /// リリースファイルのアップロード時刻
    upload_time_iso_8601: Option<String>,
    /// PEP 592: このファイルが yank されているか (欠損時は false)
    #[serde(default)]
    yanked: bool,
}

impl PyPIAdapter {
    /// 新しい PyPI アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// パッケージ用の URL を構築
    fn build_url(&self, package: &str) -> String {
        format!("{}/{}/json", PYPI_API_URL, package)
    }

    /// リリースがインストール候補になりうるか判定する (PEP 592)
    ///
    /// pip の解決挙動に合わせ、「ファイルが 0 件」または「全ファイルが yanked == true」の
    /// リリースは候補に含めない (完全に yank されたリリースは exact pin 以外で選ばれない)。
    /// 一部のファイルのみ yank された混在リリースはインストール可能なため候補に残す。
    /// crates.io / RubyGems アダプタの yanked 除外とも一貫する。
    fn is_release_installable(files: &[ReleaseInfo]) -> bool {
        !files.is_empty() && files.iter().any(|f| !f.yanked)
    }
}

#[async_trait]
impl RegistryAdapter for PyPIAdapter {
    fn language(&self) -> Language {
        Language::Python
    }

    fn registry_name(&self) -> &'static str {
        "PyPI"
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        let url = self.build_url(package);
        let response: PyPIResponse = self
            .client
            .get_json(&url, package, self.registry_name())
            .await?;

        let mut versions = Vec::new();

        for (version, release_files) in response.releases {
            // yank されたリリース (全ファイルが yanked / ファイル 0 件) をスキップ (PEP 592)
            if !Self::is_release_installable(&release_files) {
                continue;
            }

            // リリースファイルの中から最も早いアップロード時刻を取得
            let mut earliest_time: Option<DateTime<Utc>> = None;

            for file_info in release_files {
                if let Some(time_str) = &file_info.upload_time_iso_8601
                    && let Ok(time) = time_str.parse::<DateTime<Utc>>()
                {
                    earliest_time = Some(match earliest_time {
                        Some(current) if time < current => time,
                        Some(current) => current,
                        None => time,
                    });
                }
            }

            if let Some(released_at) = earliest_time {
                versions.push(VersionInfo::new(&version, released_at));
            }
        }

        // バージョンでソート
        versions.sort();

        Ok(versions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pypi_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = PyPIAdapter::new(client);
        assert_eq!(adapter.language(), Language::Python);
    }

    #[test]
    fn test_pypi_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = PyPIAdapter::new(client);
        assert_eq!(adapter.registry_name(), "PyPI");
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = PyPIAdapter::new(client);
        assert_eq!(
            adapter.build_url("requests"),
            "https://pypi.org/pypi/requests/json"
        );
    }

    #[test]
    fn test_build_url_with_dashes() {
        let client = HttpClient::new().unwrap();
        let adapter = PyPIAdapter::new(client);
        assert_eq!(
            adapter.build_url("flask-restful"),
            "https://pypi.org/pypi/flask-restful/json"
        );
    }

    #[test]
    fn test_deserialize_release_info_with_yanked() {
        let json = r#"{"upload_time_iso_8601": "2023-01-01T00:00:00Z", "yanked": true}"#;
        let info: ReleaseInfo = serde_json::from_str(json).unwrap();
        assert!(info.yanked);
    }

    #[test]
    fn test_deserialize_release_info_yanked_defaults_to_false() {
        // yanked フィールドが欠損している古い形式のレスポンスでも false 扱い
        let json = r#"{"upload_time_iso_8601": "2023-01-01T00:00:00Z"}"#;
        let info: ReleaseInfo = serde_json::from_str(json).unwrap();
        assert!(!info.yanked);
    }

    /// バグ回帰テスト (PEP 592): 全ファイルが yanked のリリースは候補から除外する。
    /// pip は完全に yank されたリリースを (exact pin 以外で) 選ばないため、
    /// depup が yank 済みバージョンへ更新提案しないようにする。
    #[test]
    fn test_fully_yanked_release_is_not_installable() {
        let json = r#"[
            {"upload_time_iso_8601": "2023-01-01T00:00:00Z", "yanked": true},
            {"upload_time_iso_8601": "2023-01-01T01:00:00Z", "yanked": true}
        ]"#;
        let files: Vec<ReleaseInfo> = serde_json::from_str(json).unwrap();
        assert!(!PyPIAdapter::is_release_installable(&files));
    }

    /// 混在リリース (一部のみ yanked) はインストール可能なため候補に残す
    #[test]
    fn test_partially_yanked_release_is_installable() {
        let json = r#"[
            {"upload_time_iso_8601": "2023-01-01T00:00:00Z", "yanked": true},
            {"upload_time_iso_8601": "2023-01-01T01:00:00Z", "yanked": false}
        ]"#;
        let files: Vec<ReleaseInfo> = serde_json::from_str(json).unwrap();
        assert!(PyPIAdapter::is_release_installable(&files));
    }

    #[test]
    fn test_release_without_files_is_not_installable() {
        // ファイルが 0 件のリリースはインストールできないため候補に含めない
        assert!(!PyPIAdapter::is_release_installable(&[]));
    }

    #[test]
    fn test_release_with_no_yanked_files_is_installable() {
        let json = r#"[{"upload_time_iso_8601": "2023-01-01T00:00:00Z", "yanked": false}]"#;
        let files: Vec<ReleaseInfo> = serde_json::from_str(json).unwrap();
        assert!(PyPIAdapter::is_release_installable(&files));
    }

    /// PyPI レスポンス全体のデシリアライズでも yanked フィールドが読まれることを確認
    #[test]
    fn test_deserialize_pypi_response_with_yanked_releases() {
        let json = r#"{
            "releases": {
                "1.0.0": [{"upload_time_iso_8601": "2023-01-01T00:00:00Z", "yanked": false}],
                "1.0.1": [{"upload_time_iso_8601": "2023-02-01T00:00:00Z", "yanked": true}]
            }
        }"#;
        let response: PyPIResponse = serde_json::from_str(json).unwrap();
        let ok_release = response.releases.get("1.0.0").unwrap();
        let yanked_release = response.releases.get("1.0.1").unwrap();
        assert!(PyPIAdapter::is_release_installable(ok_release));
        assert!(!PyPIAdapter::is_release_installable(yanked_release));
    }
}
