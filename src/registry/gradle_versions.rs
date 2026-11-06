//! Wrapper 更新に使う、Gradle 公式のリリース一覧。

use crate::error::RegistryError;
use crate::registry::HttpClient;
use crate::update::{VersionInfo, is_prerelease_version};
use chrono::{DateTime, Utc};
use serde::Deserialize;

const GRADLE_VERSIONS_URL: &str = "https://services.gradle.org/versions/all";

#[derive(Deserialize)]
struct GradleRelease {
    version: String,
    #[serde(rename = "buildTime")]
    build_time: String,
    #[serde(rename = "final", default)]
    is_final: bool,
    #[serde(default)]
    released: bool,
    #[serde(default)]
    broken: bool,
    #[serde(default)]
    snapshot: bool,
    #[serde(default)]
    nightly: bool,
    #[serde(rename = "releaseNightly", default)]
    release_nightly: bool,
}

impl GradleRelease {
    /// Wrapper の更新先の候補として一覧に残すか。
    ///
    /// 正式リリースに加えて、RC とマイルストーン (`9.9.0-rc-1` / `9.9.0-milestone-2`) も残す。
    /// プレリリースを選ぶかは judge が現在版で決める (現在版がプレリリースのときだけ残す)。
    /// 一覧から落とすと、RC を指す Wrapper の現在版が一覧に無いことになり、正式リリースが
    /// 出た後も確認が毎回失敗していた (#101)。RC は `released: false` で載るので、
    /// `released` は正式リリースにだけ求める。スナップショットとナイトリー (日時の版) と、
    /// 壊れた版は使わない。
    fn is_candidate(&self) -> bool {
        if self.snapshot || self.nightly || self.release_nightly || self.broken {
            return false;
        }
        if self.is_final {
            self.released
        } else {
            is_prerelease_version(&self.version)
        }
    }
}

fn parse_releases(body: &str) -> Result<Vec<VersionInfo>, String> {
    let releases: Vec<GradleRelease> =
        serde_json::from_str(body).map_err(|error| error.to_string())?;
    let mut versions = Vec::new();
    let mut has_final = false;
    for release in releases {
        if !release.is_candidate() {
            continue;
        }
        has_final |= release.is_final;
        let date = DateTime::parse_from_str(&release.build_time, "%Y%m%d%H%M%S%z")
            .map_err(|error| format!("{} has invalid buildTime: {error}", release.version))?
            .with_timezone(&Utc);
        versions.push(VersionInfo::new(release.version, date));
    }
    if !has_final {
        return Err("Gradle release listing contains no final releases".to_string());
    }
    Ok(versions)
}

pub(super) async fn fetch_versions(client: &HttpClient) -> Result<Vec<VersionInfo>, RegistryError> {
    let package = crate::manifest::GRADLE_WRAPPER_PACKAGE;
    let registry = "Gradle services";
    let body = client
        .get_text(GRADLE_VERSIONS_URL, package, registry)
        .await?;
    parse_releases(&body).map_err(|message| RegistryError::InvalidResponse {
        package: package.to_string(),
        registry: registry.to_string(),
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Language, UpdateResult};
    use crate::update::{UpdateFilter, UpdateJudge};

    /// services.gradle.org の一覧の形 (RC は `released: false`、マイルストーンは `released: true`。
    /// スナップショットとナイトリーは日時の版)
    const LISTING: &str = r#"[
      {"version":"9.10.0-20261106061145+0000","buildTime":"20261106061145+0000","final":false,"released":false,"snapshot":true,"nightly":true,"releaseNightly":false,"broken":false},
      {"version":"9.9.0-20261106073807+0000","buildTime":"20261106073807+0000","final":false,"released":true,"snapshot":true,"nightly":false,"releaseNightly":true,"broken":false},
      {"version":"9.9.0","buildTime":"20261101123456+0000","final":true,"released":true,"broken":false},
      {"version":"9.9.0-rc-1","buildTime":"20261020123456+0000","final":false,"released":false,"rcFor":"9.9.0","broken":false},
      {"version":"9.9.0-milestone-2","buildTime":"20261010123456+0000","final":false,"released":true,"milestoneFor":"9.9.0","broken":false},
      {"version":"9.8.0","buildTime":"20260920123456+0000","final":true,"released":true,"broken":false},
      {"version":"9.7.0","buildTime":"20260801123456+0000","final":true,"released":true,"broken":true}
    ]"#;

    #[test]
    fn listing_keeps_releases_and_prereleases_with_real_build_dates() {
        let versions = parse_releases(LISTING).unwrap();
        assert_eq!(
            versions
                .iter()
                .map(|info| info.version.as_str())
                .collect::<Vec<_>>(),
            ["9.9.0", "9.9.0-rc-1", "9.9.0-milestone-2", "9.8.0"]
        );
        assert_eq!(
            versions[3].released_at.unwrap().to_rfc3339(),
            "2026-09-20T12:34:56+00:00"
        );
        // 正式リリースが 1 つもない一覧は読めていない
        let prereleases_only = r#"[{"version":"9.9.0-rc-1","buildTime":"20261020123456+0000","final":false,"released":false,"broken":false}]"#;
        assert!(parse_releases(prereleases_only).is_err());
    }

    /// Issue #101: RC を指す Wrapper の現在版も一覧にあり、正式リリースが出ていればそこへ
    /// 更新する。正式リリースを指す Wrapper は RC やマイルストーンへ更新しない。
    #[test]
    fn a_wrapper_on_a_release_candidate_moves_to_the_final_release() {
        let properties = "distributionUrl=https\\://services.gradle.org/distributions/gradle-9.9.0-rc-1-bin.zip\n";
        let dependency = crate::manifest::parser_for(
            std::path::Path::new("gradle/wrapper/gradle-wrapper.properties"),
            Language::Java,
        )
        .parse(properties)
        .unwrap()
        .remove(0);
        let versions = parse_releases(LISTING).unwrap();
        // orchestrator は Exact の現在版が一覧に無いと確認を失敗にする
        assert!(
            versions
                .iter()
                .any(|info| info.version == dependency.version())
        );
        let now = DateTime::parse_from_rfc3339("2026-12-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let judge = UpdateJudge::with_time(UpdateFilter::new().with_include_pinned(true), now);
        assert!(matches!(
            judge.judge(&dependency, &versions),
            UpdateResult::Update { new_version, .. } if new_version == "9.9.0"
        ));
        // 正式リリースの 9.8.0 からは、新しい RC があっても正式リリースだけを候補にする
        let stable = crate::manifest::parser_for(
            std::path::Path::new("gradle/wrapper/gradle-wrapper.properties"),
            Language::Java,
        )
        .parse(&properties.replace("9.9.0-rc-1", "9.8.0"))
        .unwrap()
        .remove(0);
        let without_final: Vec<_> = versions
            .into_iter()
            .filter(|info| info.version != "9.9.0")
            .collect();
        assert!(!matches!(
            judge.judge(&stable, &without_final),
            UpdateResult::Update { .. }
        ));
    }
}
