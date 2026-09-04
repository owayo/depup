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

/// パッケージ名が PEP 503 の名前規則に沿うことを判定する
///
/// PEP 503 の正規表現 `^[A-Za-z0-9]([A-Za-z0-9._-]*[A-Za-z0-9])?$` と同義
/// (先頭・末尾は英数字、内部にのみ `.` `-` `_` を許す)。
///
/// パッケージ名は `build_url` で URL パスへ直接連結される。Poetry のテーブルキーは
/// TOML 上任意の文字列を書けるため、`"a/../lodash" = "^1.0"` のようなキーがあると
/// URL のドットセグメント正規化によって **別パッケージ** の版を取得して書き戻して
/// しまう。`?` / `#` を含む名前はクエリ・フラグメントとして解釈される。
/// Maven Central / GitHub Tags と同様に文字種で弾く (URL インジェクション防止)。
fn is_valid_pypi_package_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let (Some(first), Some(last)) = (bytes.first(), bytes.last()) else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

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

    /// パッケージ名が PEP 503 準拠であることを検証する (URL インジェクション防止)
    fn validate_package_name(&self, package: &str) -> Result<(), RegistryError> {
        if !is_valid_pypi_package_name(package) {
            return Err(RegistryError::InvalidPackageName {
                name: package.to_string(),
                registry: self.registry_name().to_string(),
                reason: "expected a PEP 503 name: [A-Za-z0-9] with inner [._-] only".to_string(),
            });
        }
        Ok(())
    }

    /// リリースファイル群から最も早いアップロード時刻を取得する
    ///
    /// `upload_time_iso_8601` を読めるファイルが 1 件も無い場合は `None` を返し、
    /// 呼び出し側で UNIX_EPOCH (= 「十分古い」) へフォールバックする。以前はリリースごと
    /// 捨てていたため、公開日不明のバージョンが無言で候補から消えて更新を取りこぼしていた。
    /// `Utc::now()` をフォールバックに使うと、デフォルト有効の age フィルタ (1w) が
    /// 「リリース直後」とみなして永久に除外してしまう。
    /// packagist / go_proxy / github_tags と同じ「日付不明 = UNIX_EPOCH」方針に揃える。
    fn earliest_upload_time(files: &[ReleaseInfo]) -> Option<DateTime<Utc>> {
        files
            .iter()
            .filter_map(|file| file.upload_time_iso_8601.as_deref())
            .filter_map(|time| time.parse::<DateTime<Utc>>().ok())
            .min()
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
        self.validate_package_name(package)?;

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
            // (全ファイルが upload_time_iso_8601 を欠く/壊れている場合は
            //  UNIX_EPOCH = 「十分古い」扱い。リリースごと捨てると公開日不明の
            //  バージョンが無言で候補から消え、有効な更新を取りこぼす)
            let released_at =
                Self::earliest_upload_time(&release_files).unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
            versions.push(VersionInfo::new(&version, released_at));
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

    /// バグ回帰テスト: `upload_time_iso_8601` を読めるファイルが 1 件も無いリリースは
    /// UNIX_EPOCH (= 十分古い) にフォールバックし、候補から無言で消えないようにする。
    /// 以前は `if let Some(...)` に else が無く、日付不明のリリースを丸ごと捨てていた。
    #[test]
    fn test_earliest_upload_time_missing_falls_back_to_epoch() {
        // upload_time_iso_8601 が欠損
        let json = r#"[{"yanked": false}]"#;
        let files: Vec<ReleaseInfo> = serde_json::from_str(json).unwrap();
        assert_eq!(PyPIAdapter::earliest_upload_time(&files), None);
        assert_eq!(
            PyPIAdapter::earliest_upload_time(&files).unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
            DateTime::<Utc>::UNIX_EPOCH
        );

        // 日付として解釈できない値
        let json = r#"[{"upload_time_iso_8601": "not-a-date", "yanked": false}]"#;
        let files: Vec<ReleaseInfo> = serde_json::from_str(json).unwrap();
        assert_eq!(PyPIAdapter::earliest_upload_time(&files), None);
    }

    #[test]
    fn test_earliest_upload_time_picks_oldest_file() {
        // 正常なエントリの日付は従来どおり「最も早いアップロード時刻」を保持する
        let json = r#"[
            {"upload_time_iso_8601": "2023-03-01T00:00:00Z", "yanked": false},
            {"upload_time_iso_8601": "2023-01-01T00:00:00Z", "yanked": false},
            {"upload_time_iso_8601": "2023-02-01T00:00:00Z", "yanked": false}
        ]"#;
        let files: Vec<ReleaseInfo> = serde_json::from_str(json).unwrap();
        assert_eq!(
            PyPIAdapter::earliest_upload_time(&files),
            Some("2023-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap())
        );
    }

    #[test]
    fn test_earliest_upload_time_ignores_unparsable_entries() {
        // 一部だけ壊れている場合は読めるものから最古を選ぶ
        let json = r#"[
            {"upload_time_iso_8601": "garbage", "yanked": false},
            {"upload_time_iso_8601": "2023-05-01T00:00:00Z", "yanked": false}
        ]"#;
        let files: Vec<ReleaseInfo> = serde_json::from_str(json).unwrap();
        assert_eq!(
            PyPIAdapter::earliest_upload_time(&files),
            Some("2023-05-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap())
        );
    }

    /// バグ回帰テスト: パッケージ名を URL パスへ連結する前に PEP 503 の名前規則で検証する。
    /// Poetry のテーブルキーは TOML 上任意の文字列を書けるため、`a/../lodash` のような
    /// キーがあると URL のドットセグメント正規化で別パッケージの版を取得してしまう。
    #[test]
    fn test_validate_package_name_rejects_url_injection() {
        let client = HttpClient::new().unwrap();
        let adapter = PyPIAdapter::new(client);
        for invalid in [
            "",
            "a/../lodash",
            "a/b",
            "x?y=1",
            "x#frag",
            "..",
            ".",
            ".hidden",
            "trailing-",
            "with space",
            "requests\n",
            "パッケージ",
        ] {
            let err = adapter.validate_package_name(invalid).unwrap_err();
            assert!(
                matches!(err, RegistryError::InvalidPackageName { .. }),
                "expected InvalidPackageName for {:?}, got {:?}",
                invalid,
                err
            );
        }
    }

    #[test]
    fn test_validate_package_name_accepts_pep503_names() {
        let client = HttpClient::new().unwrap();
        let adapter = PyPIAdapter::new(client);
        for valid in [
            "requests",
            "Django",
            "zope.interface",
            "ruamel.yaml",
            "flask-restful",
            "typing_extensions",
            "backports.zoneinfo",
            "a",
            "2to3",
        ] {
            assert!(
                adapter.validate_package_name(valid).is_ok(),
                "expected {:?} to be accepted",
                valid
            );
        }
    }
}
