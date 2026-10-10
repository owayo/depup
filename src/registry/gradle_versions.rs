//! Official Gradle release listing for Wrapper updates.

use crate::error::RegistryError;
use crate::registry::HttpClient;
use crate::update::VersionInfo;
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
}

fn parse_releases(body: &str) -> Result<Vec<VersionInfo>, String> {
    let releases: Vec<GradleRelease> =
        serde_json::from_str(body).map_err(|error| error.to_string())?;
    let mut versions = Vec::new();
    for release in releases {
        if !release.is_final || !release.released || release.broken {
            continue;
        }
        let date = DateTime::parse_from_str(&release.build_time, "%Y%m%d%H%M%S%z")
            .map_err(|error| format!("{} has invalid buildTime: {error}", release.version))?
            .with_timezone(&Utc);
        versions.push(VersionInfo::new(release.version, date));
    }
    if versions.is_empty() {
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

    #[test]
    fn final_releases_have_real_build_dates() {
        let body = r#"[
          {"version":"9.8.0","buildTime":"20260920123456+0000","final":true,"released":true,"broken":false},
          {"version":"9.9-rc-1","buildTime":"20261001123456+0000","final":false,"released":true,"broken":false},
          {"version":"9.7.0","buildTime":"20260801123456+0000","final":true,"released":true,"broken":true}
        ]"#;
        let versions = parse_releases(body).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, "9.8.0");
        assert_eq!(
            versions[0].released_at.unwrap().to_rfc3339(),
            "2026-09-20T12:34:56+00:00"
        );
    }
}
