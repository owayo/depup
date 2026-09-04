//! Go Module Proxy アダプタ
//!
//! Go Module Proxy からモジュールバージョン情報を取得する。
//! API エンドポイント:
//! - バージョン一覧: https://proxy.golang.org/{module}/@v/list
//! - バージョン情報: https://proxy.golang.org/{module}/@v/{version}.info
//! - go.mod: https://proxy.golang.org/{module}/@v/{version}.mod
//!
//! `@v/list` は Go 本体が候補として採用しない版も含むため、そのまま使わずに
//! `+incompatible` の選別 (`filter_incompatible_versions`) と retract の除外を
//! 適用してから候補にする。

use crate::domain::Language;
use crate::error::RegistryError;
use crate::manifest::GoModParser;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::{VersionInfo, compare_semver_versions, is_prerelease_version};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt};
use serde::Deserialize;

/// Go Module Proxy のベース URL
const GO_PROXY_URL: &str = "https://proxy.golang.org";

/// `@v/{version}.info` を同時に取得する本数。
///
/// Go proxy はバージョン一覧しか一括で返さず、リリース時刻は版ごとに 1 リクエスト
/// 必要になる。版数が多いモジュール (`github.com/aws/aws-sdk-go` で 1865 件) を
/// 直列に引くと 1 依存だけで数分かかるため並列化する。マニフェスト側の並列度
/// (依存数に応じて最大 4) と掛け合わさるので、控えめな値にしている。
const INFO_FETCH_CONCURRENCY: usize = 8;

/// Go Module Proxy アダプタ
pub struct GoProxyAdapter {
    client: HttpClient,
}

/// バージョン情報レスポンス
#[derive(Debug, Deserialize)]
struct VersionInfoResponse {
    /// バージョン文字列
    #[serde(rename = "Version")]
    version: String,
    /// バージョンが作成された時刻
    #[serde(rename = "Time")]
    time: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Retraction {
    Exact(String),
    Range { lower: String, upper: String },
}

impl GoProxyAdapter {
    /// 新しい Go Proxy アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// バージョン一覧用の URL を構築
    fn build_list_url(&self, module: &str) -> String {
        // モジュールパスを URL エンコード (大文字小文字を区別しない検索のため)
        let encoded_module = Self::encode_module_path(module);
        format!("{}/@v/list", encoded_module)
    }

    /// バージョン情報用の URL を構築
    ///
    /// Go Module Proxy プロトコルは `$module` と `$version` の両方を case-encode するため
    /// (https://go.dev/ref/mod#goproxy-protocol)、バージョンにも適用する。
    /// 適用しないと `v1.0.0-RC1` のような大文字入りバージョンの `.info` 取得が
    /// 404 になり、そのバージョンが候補から silent に欠落する。
    fn build_info_url(&self, module: &str, version: &str) -> String {
        let encoded_module = Self::encode_module_path(module);
        format!("{}/@v/{}.info", encoded_module, Self::case_encode(version))
    }

    /// 指定バージョンの `go.mod` 取得 URL を構築する。
    fn build_mod_url(&self, module: &str, version: &str) -> String {
        let encoded_module = Self::encode_module_path(module);
        format!("{}/@v/{}.mod", encoded_module, Self::case_encode(version))
    }

    /// タグ付きバージョンがないモジュール向けの `@latest` URL を構築する。
    fn build_latest_url(&self, module: &str) -> String {
        format!("{}/@latest", Self::encode_module_path(module))
    }

    /// Go Proxy URL 用にモジュールパスをエンコード
    fn encode_module_path(module: &str) -> String {
        format!("{}/{}", GO_PROXY_URL, Self::case_encode(module))
    }

    /// Go Module Proxy プロトコルの case-encoding (ASCII 大文字 → `!` + 小文字)
    ///
    /// 大文字小文字を区別しないファイルシステム上での曖昧さを避けるためのエンコードで、
    /// モジュールパスとバージョン文字列の両方に適用される。
    /// Go の仕様では ASCII 大文字のみが対象なので `is_ascii_uppercase` で判定する
    /// (Unicode 大文字まで変換すると仕様外のエンコードになる)。
    fn case_encode(s: &str) -> String {
        let mut encoded = String::with_capacity(s.len());

        for ch in s.chars() {
            if ch.is_ascii_uppercase() {
                encoded.push('!');
                encoded.push(ch.to_ascii_lowercase());
            } else {
                encoded.push(ch);
            }
        }

        encoded
    }
}

/// プロキシのバージョン一覧から、retract 情報を保持する生の最新バージョンを選ぶ。
///
/// Go の `@latest` と同じく、安定版が1件でもあれば最上位の安定版を選び、
/// 安定版がなければ最上位のプレリリースを選ぶ。retract 適用前の一覧を使うため、
/// 最新版が自身を retract している場合も、その `go.mod` を情報源にできる。
fn latest_version_for_retractions<'a>(
    versions: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut latest_release: Option<&str> = None;
    let mut latest_prerelease: Option<&str> = None;

    for version in versions {
        let latest = if is_prerelease_version(version) {
            &mut latest_prerelease
        } else {
            &mut latest_release
        };
        let should_replace = match *latest {
            Some(current) => {
                compare_semver_versions(version, current) == std::cmp::Ordering::Greater
            }
            None => true,
        };
        if should_replace {
            *latest = Some(version);
        }
    }

    latest_release.or(latest_prerelease)
}

/// バージョンが Go の `+incompatible` 版かどうかを判定する。
///
/// Go 本体 (`modload.filterVersions`) と同じく末尾一致で判定する。
fn is_incompatible_version(version: &str) -> bool {
    version.ends_with("+incompatible")
}

/// semver 昇順で並べたときに、最初の `+incompatible` の直前へ来る compatible 版を返す。
///
/// これが Go の `filterVersions` が go.mod の有無を問い合わせる `lastCompatible`。
/// `+incompatible` が 1 件も無い場合、および `+incompatible` より前に compatible 版が
/// 無い場合は `None` を返す (どちらも問い合わせ不要で、後者は `+incompatible` しか
/// 選択肢が無いのでそのまま残す)。
fn last_compatible_before_incompatible<'a>(versions: &[&'a str]) -> Option<&'a str> {
    let mut sorted: Vec<&'a str> = versions.to_vec();
    sorted.sort_by(|a, b| compare_semver_versions(a, b));

    let mut last_compatible: Option<&'a str> = None;
    for version in sorted {
        if is_incompatible_version(version) {
            return last_compatible;
        }
        last_compatible = Some(version);
    }

    None
}

/// Go 本体の `modload.filterVersions` 相当の `+incompatible` 選別。
///
/// `@v/list` は 2019 年の修正 (golang.org/issue/34165) 以前にキャッシュされた
/// `+incompatible` タグを保持し続けるため、一覧をそのまま候補にすると
/// `github.com/libp2p/go-libp2p` が `v0.49.0` から 2018 年の `v6.0.23+incompatible` へ、
/// `github.com/russross/blackfriday` が `v1.6.0` から `v2.0.0+incompatible` へ
/// 「更新」される。`+incompatible` は prerelease ではなく build metadata なので
/// prerelease フィルタで落ちず、リリース日も古いので age フィルタも通り抜ける。
/// クライアント側のこの選別が唯一の防波堤になる。
///
/// 規則: semver 昇順で走査し、最初の `+incompatible` に到達した時点で直前の
/// compatible 版が本物の go.mod を持つなら、そこから先 (昇順なので以降はすべて
/// `+incompatible`) を候補から落とす。go.mod を持たないなら作者がモジュール以前の
/// major タグ運用をしているとみなし、`+incompatible` を残す。
///
/// `last_compatible_has_go_mod` は [`last_compatible_before_incompatible`] が返した
/// 版の go.mod 実在フラグ。判定対象が無い (`None`) 場合は `false` を渡す。
///
/// 制限: Go は現在版が `+incompatible` のとき (`preferIncompatible`) 全候補を許可するが、
/// `RegistryAdapter::fetch_versions` は現在版を受け取らないためこの分岐は再現できない。
/// その結果 `+incompatible` を使っているプロジェクトは更新候補が現在版より低くなり
/// 「更新なし」になる。ダウングレードを書き込むよりは安全側と判断している。
fn filter_incompatible_versions<'a>(
    versions: &[&'a str],
    last_compatible_has_go_mod: bool,
) -> Vec<&'a str> {
    if !last_compatible_has_go_mod {
        return versions.to_vec();
    }

    let mut order: Vec<usize> = (0..versions.len()).collect();
    order.sort_by(|&a, &b| compare_semver_versions(versions[a], versions[b]));

    let mut keep = vec![true; versions.len()];
    let mut seen_compatible = false;
    for (position, &index) in order.iter().enumerate() {
        if !is_incompatible_version(versions[index]) {
            seen_compatible = true;
            continue;
        }
        // 最初の `+incompatible` に到達。Go はここで走査を打ち切る。
        if seen_compatible {
            for &dropped in &order[position..] {
                keep[dropped] = false;
            }
        }
        break;
    }

    versions
        .iter()
        .zip(keep)
        .filter_map(|(version, keep)| keep.then_some(*version))
        .collect()
}

/// プロキシが合成した go.mod (= 本物の go.mod を持たない版) かどうかを判定する。
///
/// Go の `versionHasGoMod` は取得内容を `modfetch.LegacyGoMod`
/// (`module <path>` の 1 行だけ) とバイト比較する。ここでは行末・空行の差異だけを
/// 吸収し、それ以外は「本物の go.mod」として扱う。合成と誤判定すると
/// `+incompatible` を残してしまい、防ごうとしているダウングレードがそのまま起きるため、
/// 判定は安全側 (= 本物寄り) に倒す。
fn is_synthesized_go_mod(content: &str, module_path: &str) -> bool {
    let mut lines = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());

    let Some(first) = lines.next() else {
        return false;
    };
    if lines.next().is_some() {
        return false;
    }

    let Some(rest) = first.strip_prefix("module") else {
        return false;
    };
    if !rest.starts_with(char::is_whitespace) {
        return false;
    }

    // `modfile.AutoQuote` は必要なときだけ引用符を付けるため、両形式を受け付ける。
    let declared = rest.trim();
    let declared = declared
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(declared);

    declared == module_path
}

/// `go.mod` から単一版・閉区間の retract 指示を抽出する。
fn parse_retractions(content: &str) -> Vec<Retraction> {
    let mut retractions = Vec::new();
    let mut in_retract_block = false;

    for line in content.lines() {
        let logical = line.split("//").next().unwrap_or("").trim();
        if logical.is_empty() {
            continue;
        }

        // ブロック開始判定は go.mod パーサと共有する。以前は `starts_with("retract (")`
        // の文字列前方一致だったため、`retract(` (空白なし) のブロックを認識できず
        // 撤回済み版が候補に残っていた。
        if GoModParser::is_block_start(logical, "retract") {
            in_retract_block = true;
            continue;
        }
        if in_retract_block && logical == ")" {
            in_retract_block = false;
            continue;
        }

        let spec = if in_retract_block {
            logical
        } else if let Some(rest) = logical.strip_prefix("retract")
            && rest.starts_with(char::is_whitespace)
        {
            rest.trim()
        } else {
            continue;
        };

        if let Some(retraction) = parse_retraction_spec(spec) {
            retractions.push(retraction);
        }
    }

    retractions
}

/// retract の単一バージョンまたは閉区間を解釈する。
fn parse_retraction_spec(spec: &str) -> Option<Retraction> {
    let spec = spec.trim();
    if let Some(inner) = spec
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        let (lower, upper) = inner.split_once(',')?;
        let lower = unquote_version(lower.trim())?;
        let upper = unquote_version(upper.trim())?;
        return Some(Retraction::Range { lower, upper });
    }

    unquote_version(spec).map(Retraction::Exact)
}

/// go.mod の ident、二重引用符、raw string で記述されたバージョンを取り出す。
fn unquote_version(version: &str) -> Option<String> {
    let version = version.trim();
    if version.is_empty() {
        return None;
    }
    let is_quoted = version.len() >= 2
        && ((version.starts_with('"') && version.ends_with('"'))
            || (version.starts_with('`') && version.ends_with('`')));
    let unquoted = if is_quoted {
        &version[1..version.len() - 1]
    } else {
        version
    };
    (!unquoted.is_empty()).then(|| unquoted.to_string())
}

/// バージョンが retract の単一版または包含範囲に該当するか判定する。
fn is_retracted(version: &str, retractions: &[Retraction]) -> bool {
    retractions.iter().any(|retraction| match retraction {
        Retraction::Exact(retracted) => {
            compare_semver_versions(version, retracted) == std::cmp::Ordering::Equal
        }
        Retraction::Range { lower, upper } => {
            compare_semver_versions(version, lower) != std::cmp::Ordering::Less
                && compare_semver_versions(version, upper) != std::cmp::Ordering::Greater
        }
    })
}

/// Go Proxy の `.info` 応答を共通のバージョン情報へ変換する。
///
/// `Time` はプロキシ仕様上省略可能なため、欠落または不正値なら UNIX epoch を使い、
/// `--age` によって日付不明の正当なバージョンが除外されないようにする。
fn into_version_info(info: VersionInfoResponse) -> VersionInfo {
    let released_at = info
        .time
        .as_deref()
        .and_then(|time| time.parse::<DateTime<Utc>>().ok())
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
    VersionInfo::new(info.version, released_at)
}

#[async_trait]
impl RegistryAdapter for GoProxyAdapter {
    fn language(&self) -> Language {
        Language::Go
    }

    fn registry_name(&self) -> &'static str {
        "Go Proxy"
    }

    async fn fetch_versions(&self, module: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        // まずバージョン一覧を取得
        let list_url = self.build_list_url(module);
        let version_list = self
            .client
            .get_text(&list_url, module, self.registry_name())
            .await?;

        let version_strings: Vec<&str> = version_list
            .lines()
            .map(str::trim)
            .filter(|version| !version.is_empty())
            .collect();

        if version_strings.is_empty() {
            let latest_url = self.build_latest_url(module);
            let latest = self
                .client
                .get_json::<VersionInfoResponse>(&latest_url, module, self.registry_name())
                .await?;
            let mod_url = self.build_mod_url(module, &latest.version);
            let latest_go_mod = self
                .client
                .get_text(&mod_url, module, self.registry_name())
                .await?;
            let retractions = parse_retractions(&latest_go_mod);
            return if is_retracted(&latest.version, &retractions) {
                Ok(Vec::new())
            } else {
                Ok(vec![into_version_info(latest)])
            };
        }

        // `+incompatible` を候補に残すかどうかを Go 本体と同じ規則で決める。
        // 判定に使う go.mod は、後段の retract 情報源と同じ版になることが多い
        // (どちらも「最も新しい compatible 版」) ので取得結果を控えて使い回す。
        let mut cached_go_mod: Option<(&str, String)> = None;
        let mut last_compatible_has_go_mod = false;
        if let Some(last_compatible) = last_compatible_before_incompatible(&version_strings) {
            let mod_url = self.build_mod_url(module, last_compatible);
            match self
                .client
                .get_text(&mod_url, module, self.registry_name())
                .await
            {
                Ok(go_mod) => {
                    last_compatible_has_go_mod = !is_synthesized_go_mod(&go_mod, module);
                    cached_go_mod = Some((last_compatible, go_mod));
                }
                Err(_) => {
                    // 判定材料が取れないときは `+incompatible` を落とす側へ倒す。
                    // 取りこぼしは「更新なし」として利用者に見えるだけだが、逆側に
                    // 倒すと 8 年前の `+incompatible` 版へのダウングレードが
                    // 「更新」として黙って書き込まれる。
                    last_compatible_has_go_mod = true;
                }
            }
        }

        let candidates = filter_incompatible_versions(&version_strings, last_compatible_has_go_mod);

        // retract 情報源は `+incompatible` を落とした後の一覧から選ぶ。フィルタ前から
        // 選ぶと `github.com/libp2p/go-libp2p` では `v6.0.23+incompatible` の
        // 合成 go.mod を読んでしまい、`v0.49.0` の go.mod にある本物の retract を
        // 取りこぼす。
        let latest_version = latest_version_for_retractions(candidates.iter().copied())
            .ok_or_else(|| RegistryError::InvalidResponse {
                package: module.to_string(),
                registry: self.registry_name().to_string(),
                message: "retract 情報を取得する最新バージョンを決定できません".to_string(),
            })?;
        let latest_go_mod = match cached_go_mod {
            Some((version, go_mod)) if version == latest_version => go_mod,
            _ => {
                let mod_url = self.build_mod_url(module, latest_version);
                self.client
                    .get_text(&mod_url, module, self.registry_name())
                    .await?
            }
        };
        let retractions = parse_retractions(&latest_go_mod);

        // 各バージョンについて、リリース時刻を取得するために情報をフェッチする。
        //
        // `@v/list` は数千件になりうる (`github.com/aws/aws-sdk-go` で 1865 件)。
        // 1 件ずつ await すると 1 依存の解決だけで数分の無音待ちになり、利用者からは
        // ハングと区別がつかない。しかも取得失敗時は 1 件ごとにリトライの
        // 総デッドラインまで待たされる。結果は最後にソートし直すので取得順に
        // 意味は無く、`buffer_unordered` で並列化する。
        // URL は事前に組み立てて所有値で持つ。ストリームへ `&str` を流すと
        // クロージャが特定のライフタイムに束縛され、`buffer_unordered` が要求する
        // 高階ライフタイム境界 (どのライフタイムでも成立する `FnMut`) を満たせない。
        let info_urls: Vec<String> = candidates
            .into_iter()
            .filter(|version| !is_retracted(version, &retractions))
            .map(|version| self.build_info_url(module, version))
            .collect();

        // 同じ理由で `self` も async ブロックへ持ち込まず、必要な参照だけ取り出す。
        let client = &self.client;
        let registry_name = self.registry_name();

        let mut versions: Vec<VersionInfo> = stream::iter(info_urls)
            .map(|info_url| async move {
                // 特定バージョンの情報が取得できない場合はスキップ
                client
                    .get_json::<VersionInfoResponse>(&info_url, module, registry_name)
                    .await
                    .ok()
                    .map(into_version_info)
            })
            .buffer_unordered(INFO_FETCH_CONCURRENCY)
            .filter_map(std::future::ready)
            .collect()
            .await;

        // バージョンでソート
        versions.sort();

        Ok(versions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_go_proxy_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(adapter.language(), Language::Go);
    }

    #[test]
    fn test_go_proxy_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(adapter.registry_name(), "Go Proxy");
    }

    #[test]
    fn test_encode_module_path_simple() {
        assert_eq!(
            GoProxyAdapter::encode_module_path("github.com/gin-gonic/gin"),
            "https://proxy.golang.org/github.com/gin-gonic/gin"
        );
    }

    #[test]
    fn test_encode_module_path_with_uppercase() {
        // 大文字は !小文字 にエンコードされるべき
        assert_eq!(
            GoProxyAdapter::encode_module_path("github.com/Azure/azure-sdk-for-go"),
            "https://proxy.golang.org/github.com/!azure/azure-sdk-for-go"
        );
    }

    #[test]
    fn test_build_list_url() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_list_url("github.com/gin-gonic/gin"),
            "https://proxy.golang.org/github.com/gin-gonic/gin/@v/list"
        );
    }

    #[test]
    fn test_build_info_url() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_info_url("github.com/gin-gonic/gin", "v1.9.0"),
            "https://proxy.golang.org/github.com/gin-gonic/gin/@v/v1.9.0.info"
        );
    }

    /// バグ回帰テスト: Go Module Proxy プロトコルは `$version` も case-encode する。
    /// 以前はモジュールパスにしか適用していなかったため、`v1.0.0-RC1` のような
    /// 大文字入りバージョンの `.info` 取得が 404 になり候補から silent に欠落していた。
    #[test]
    fn test_case_encode_version_with_uppercase() {
        assert_eq!(GoProxyAdapter::case_encode("v1.0.0-RC1"), "v1.0.0-!r!c1");
    }

    #[test]
    fn test_case_encode_lowercase_unchanged() {
        assert_eq!(GoProxyAdapter::case_encode("v1.9.0"), "v1.9.0");
        assert_eq!(
            GoProxyAdapter::case_encode("v1.2.3-beta.1"),
            "v1.2.3-beta.1"
        );
    }

    #[test]
    fn test_build_info_url_encodes_version_case() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_info_url("github.com/gin-gonic/gin", "v1.0.0-RC1"),
            "https://proxy.golang.org/github.com/gin-gonic/gin/@v/v1.0.0-!r!c1.info"
        );
    }

    #[test]
    fn test_build_info_url_encodes_both_module_and_version() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_info_url("github.com/Azure/azure-sdk-for-go", "v1.0.0-RC1"),
            "https://proxy.golang.org/github.com/!azure/azure-sdk-for-go/@v/v1.0.0-!r!c1.info"
        );
    }

    #[test]
    fn test_build_mod_url_encodes_both_module_and_version() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_mod_url("github.com/Azure/azure-sdk-for-go", "v1.0.0-RC1"),
            "https://proxy.golang.org/github.com/!azure/azure-sdk-for-go/@v/v1.0.0-!r!c1.mod"
        );
    }

    #[test]
    fn test_build_latest_url_encodes_module() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_latest_url("github.com/Azure/azure-sdk-for-go"),
            "https://proxy.golang.org/github.com/!azure/azure-sdk-for-go/@latest"
        );
    }

    #[test]
    fn test_info_without_time_uses_unix_epoch() {
        let response: VersionInfoResponse =
            serde_json::from_str(r#"{"Version":"v0.0.0-20240101000000-abcdef123456"}"#).unwrap();
        let info = into_version_info(response);

        assert_eq!(info.version, "v0.0.0-20240101000000-abcdef123456");
        assert_eq!(info.released_at, DateTime::<Utc>::UNIX_EPOCH);
    }

    #[test]
    fn test_info_with_invalid_time_uses_unix_epoch() {
        let response = VersionInfoResponse {
            version: "v1.0.0".to_string(),
            time: Some("not-a-date".to_string()),
        };
        let info = into_version_info(response);

        assert_eq!(info.released_at, DateTime::<Utc>::UNIX_EPOCH);
    }

    #[test]
    fn test_latest_version_for_retractions_prefers_release() {
        let versions = ["v1.9.0", "v2.0.0-rc.1", "v1.10.0"];
        assert_eq!(latest_version_for_retractions(versions), Some("v1.10.0"));
    }

    #[test]
    fn test_latest_version_for_retractions_uses_prerelease_without_release() {
        let versions = ["v2.0.0-beta.1", "v2.0.0-rc.1"];
        assert_eq!(
            latest_version_for_retractions(versions),
            Some("v2.0.0-rc.1")
        );
    }

    #[test]
    fn test_parse_retractions_supports_single_block_range_and_quotes() {
        let content = r#"
module example.com/lib

retract v1.0.0 // 誤って公開した版
retract (
    [v1.1.0, v1.2.0]
    "v1.3.0"
    [`v1.4.0`, "v1.5.0"]
)
"#;

        assert_eq!(
            parse_retractions(content),
            vec![
                Retraction::Exact("v1.0.0".to_string()),
                Retraction::Range {
                    lower: "v1.1.0".to_string(),
                    upper: "v1.2.0".to_string(),
                },
                Retraction::Exact("v1.3.0".to_string()),
                Retraction::Range {
                    lower: "v1.4.0".to_string(),
                    upper: "v1.5.0".to_string(),
                },
            ]
        );
    }

    /// バグ回帰テスト: `github.com/libp2p/go-libp2p` の実データ。
    /// `@v/list` の最大安定版は 2018 年の `v6.0.23+incompatible` だが、
    /// `v0.49.0` が本物の go.mod を持つため Go は `+incompatible` を全部捨てる。
    /// 以前は選別が無く、`v0.49.0` から 8 年前の版へ「更新」していた。
    #[test]
    fn test_filter_incompatible_drops_when_last_compatible_has_go_mod() {
        let versions = [
            "v0.48.0",
            "v2.0.0+incompatible",
            "v0.49.0",
            "v6.0.23+incompatible",
        ];

        assert_eq!(
            last_compatible_before_incompatible(&versions),
            Some("v0.49.0")
        );
        assert_eq!(
            filter_incompatible_versions(&versions, true),
            vec!["v0.48.0", "v0.49.0"]
        );
    }

    /// 直前の compatible 版が合成 go.mod しか持たない (= モジュール以前の major タグ運用)
    /// なら、Go と同じく `+incompatible` を候補に残す。
    #[test]
    fn test_filter_incompatible_keeps_when_last_compatible_lacks_go_mod() {
        let versions = [
            "v0.48.0",
            "v2.0.0+incompatible",
            "v0.49.0",
            "v6.0.23+incompatible",
        ];

        assert_eq!(
            filter_incompatible_versions(&versions, false),
            versions.to_vec()
        );
    }

    /// `+incompatible` しか無い一覧では判定対象の compatible 版が存在しないため、
    /// 候補が全滅しないよう `+incompatible` をそのまま残す。
    #[test]
    fn test_filter_incompatible_keeps_incompatible_only_list() {
        let versions = ["v3.0.0+incompatible", "v2.0.0+incompatible"];

        assert_eq!(last_compatible_before_incompatible(&versions), None);
        // 判定対象が無いので `false` を渡す運用だが、`true` でも落とさないこと
        assert_eq!(
            filter_incompatible_versions(&versions, false),
            versions.to_vec()
        );
        assert_eq!(
            filter_incompatible_versions(&versions, true),
            versions.to_vec()
        );
    }

    /// `+incompatible` を含まない一覧は素通しする。
    #[test]
    fn test_filter_incompatible_keeps_compatible_only_list() {
        let versions = ["v1.6.0", "v1.5.3", "v2.0.0-rc.1"];

        assert_eq!(last_compatible_before_incompatible(&versions), None);
        assert_eq!(
            filter_incompatible_versions(&versions, true),
            versions.to_vec()
        );
    }

    /// Go は compatible なプレリリースを `+incompatible` リリースより優先する
    /// (「we even prefer a compatible pre-release over an incompatible release」)。
    #[test]
    fn test_filter_incompatible_prefers_compatible_prerelease() {
        let versions = ["v1.6.0", "v1.7.0-rc.1", "v2.0.0+incompatible"];

        assert_eq!(
            last_compatible_before_incompatible(&versions),
            Some("v1.7.0-rc.1")
        );
        assert_eq!(
            filter_incompatible_versions(&versions, true),
            vec!["v1.6.0", "v1.7.0-rc.1"]
        );
    }

    /// バグ回帰テスト (副次): retract 情報源をフィルタ前の一覧から選ぶと、
    /// `v6.0.23+incompatible` の合成 go.mod を読んで本物の retract を取りこぼす。
    #[test]
    fn test_latest_version_for_retractions_uses_filtered_candidates() {
        let versions = ["v0.49.0", "v6.0.23+incompatible"];

        // `+incompatible` は build metadata なので prerelease 扱いされず最大版になる
        assert_eq!(
            latest_version_for_retractions(versions),
            Some("v6.0.23+incompatible")
        );

        let candidates = filter_incompatible_versions(&versions, true);
        assert_eq!(
            latest_version_for_retractions(candidates.iter().copied()),
            Some("v0.49.0")
        );
    }

    #[test]
    fn test_is_synthesized_go_mod_detects_legacy_go_mod() {
        // プロキシが合成する go.mod は `module <path>` の 1 行だけ
        assert!(is_synthesized_go_mod(
            "module github.com/libp2p/go-libp2p\n",
            "github.com/libp2p/go-libp2p"
        ));
        // CRLF・前後の空行は差異として扱わない
        assert!(is_synthesized_go_mod(
            "\r\nmodule github.com/x/y\r\n\r\n",
            "github.com/x/y"
        ));
        // 引用符付きの module 行も合成形
        assert!(is_synthesized_go_mod(
            "module \"github.com/x/y\"\n",
            "github.com/x/y"
        ));
    }

    #[test]
    fn test_is_synthesized_go_mod_treats_ambiguous_content_as_real() {
        // `go` 行などがあれば本物の go.mod
        assert!(!is_synthesized_go_mod(
            "module github.com/x/y\n\ngo 1.21\n",
            "github.com/x/y"
        ));
        // 別モジュールの go.mod は合成形ではない
        assert!(!is_synthesized_go_mod(
            "module github.com/other/mod\n",
            "github.com/x/y"
        ));
        // 判定できない入力は安全側 (本物扱い = `+incompatible` を落とす) へ倒す
        assert!(!is_synthesized_go_mod("", "github.com/x/y"));
        assert!(!is_synthesized_go_mod(
            "modulegithub.com/x/y\n",
            "github.com/x/y"
        ));
    }

    /// バグ回帰テスト: `retract(` (空白なし) のブロックを認識できず、
    /// 撤回済み版が候補に残っていた。
    #[test]
    fn test_parse_retractions_supports_block_without_space() {
        let content = r#"
module example.com/lib

retract(
    v1.0.0
    [v1.1.0, v1.2.0]
)
"#;

        assert_eq!(
            parse_retractions(content),
            vec![
                Retraction::Exact("v1.0.0".to_string()),
                Retraction::Range {
                    lower: "v1.1.0".to_string(),
                    upper: "v1.2.0".to_string(),
                },
            ]
        );
    }

    /// `retract` とバージョンがタブ区切りの単一行指定も解釈する。
    #[test]
    fn test_parse_retractions_supports_tab_separated_single_line() {
        assert_eq!(
            parse_retractions("module example.com/lib\n\nretract\tv1.0.0\n"),
            vec![Retraction::Exact("v1.0.0".to_string())]
        );
    }

    /// `retract` を接頭辞に持つだけの別ディレクティブは拾わない。
    #[test]
    fn test_parse_retractions_ignores_similar_directive() {
        assert!(parse_retractions("retracted v1.0.0\nretractions (\n)\n").is_empty());
    }

    #[test]
    fn test_is_retracted_matches_exact_and_inclusive_range() {
        let retractions = vec![
            Retraction::Exact("v1.0.0".to_string()),
            Retraction::Range {
                lower: "v1.2.0".to_string(),
                upper: "v1.4.0".to_string(),
            },
        ];

        assert!(is_retracted("1.0.0", &retractions));
        assert!(is_retracted("v1.2.0", &retractions));
        assert!(is_retracted("v1.3.0", &retractions));
        assert!(is_retracted("v1.4.0", &retractions));
        assert!(!is_retracted("v1.1.0", &retractions));
        assert!(!is_retracted("v1.4.1", &retractions));
    }
}
