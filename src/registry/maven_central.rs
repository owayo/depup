//! Maven Central repository metadata adapter.
//!
//! The search index can lag behind the repository. Artifact-level Maven metadata is the
//! authoritative version list; individual POM responses provide release dates.

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter, is_valid_registry_id_segment};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::header::LAST_MODIFIED;
use serde::Deserialize;

const MAVEN_CENTRAL_URL: &str = "https://repo1.maven.org/maven2";

pub struct MavenCentralAdapter {
    client: HttpClient,
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct MavenMetadata {
    #[serde(rename = "groupId")]
    group_id: String,
    #[serde(rename = "artifactId")]
    artifact_id: String,
    versioning: MavenVersioning,
}

#[derive(Debug, Deserialize)]
struct MavenVersioning {
    versions: MavenVersions,
}

#[derive(Debug, Deserialize)]
struct MavenVersions {
    #[serde(rename = "version", default)]
    version: Vec<String>,
}

impl MavenCentralAdapter {
    pub fn new(client: HttpClient) -> Self {
        Self {
            client,
            base_url: MAVEN_CENTRAL_URL.to_string(),
        }
    }

    fn artifact_url(&self, package: &str) -> Result<String, RegistryError> {
        let Some((group, artifact)) = package.split_once(':') else {
            return Err(self.invalid_package(package));
        };
        if artifact.contains(':')
            || !is_valid_registry_id_segment(group)
            || !is_valid_registry_id_segment(artifact)
        {
            return Err(self.invalid_package(package));
        }
        Ok(format!(
            "{}/{}/{}",
            self.base_url,
            group.replace('.', "/"),
            artifact
        ))
    }

    fn invalid_package(&self, package: &str) -> RegistryError {
        RegistryError::InvalidPackageName {
            name: package.to_string(),
            registry: self.registry_name().to_string(),
            reason: "expected a safe groupId:artifactId coordinate".to_string(),
        }
    }

    fn version_url(&self, package: &str, version: &str) -> Result<String, RegistryError> {
        let artifact_url = self.artifact_url(package)?;
        if version.is_empty()
            || matches!(version, "." | "..")
            || !version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+'))
        {
            return Err(RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: self.registry_name().to_string(),
                message: format!("unsafe Maven version: {version}"),
            });
        }
        let artifact = package.split_once(':').expect("validated coordinate").1;
        Ok(format!("{artifact_url}/{version}/{artifact}-{version}.pom"))
    }
}

#[async_trait]
impl RegistryAdapter for MavenCentralAdapter {
    fn language(&self) -> Language {
        Language::Java
    }

    fn registry_name(&self) -> &'static str {
        "Maven Central"
    }

    fn release_dates_deferred(&self) -> bool {
        true
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        let url = format!("{}/maven-metadata.xml", self.artifact_url(package)?);
        let xml = self
            .client
            .get_text(&url, package, self.registry_name())
            .await?;
        let metadata: MavenMetadata =
            quick_xml::de::from_str(&xml).map_err(|error| RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: self.registry_name().to_string(),
                message: format!("Maven metadata XML: {error}"),
            })?;
        let (group, artifact) = package.split_once(':').expect("validated coordinate");
        if metadata.group_id != group || metadata.artifact_id != artifact {
            return Err(RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: self.registry_name().to_string(),
                message: "Maven metadata coordinate does not match request".to_string(),
            });
        }

        // The listing has no per-version timestamps. A conservative placeholder is
        // replaced for each candidate before the judge makes a final decision.
        let unknown_date = Utc.timestamp_opt(0, 0).single().expect("Unix epoch");
        let mut versions = metadata
            .versioning
            .versions
            .version
            .into_iter()
            .map(|version| VersionInfo::new(version, unknown_date))
            .collect::<Vec<_>>();
        versions.sort();
        Ok(versions)
    }

    async fn fetch_release_date(
        &self,
        package: &str,
        version: &str,
    ) -> Result<Option<DateTime<Utc>>, RegistryError> {
        let url = self.version_url(package, version)?;
        let response = self
            .client
            .get_response_with_retry(
                || self.client.inner().head(&url),
                package,
                self.registry_name(),
            )
            .await?;
        if let Some(error) = crate::registry::client::map_status_error(
            response.status(),
            package,
            self.registry_name(),
        ) {
            return Err(error);
        }
        let header = response.headers().get(LAST_MODIFIED).ok_or_else(|| {
            RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: self.registry_name().to_string(),
                message: format!("POM for {version} has no Last-Modified header"),
            }
        })?;
        let date = DateTime::parse_from_rfc2822(header.to_str().unwrap_or(""))
            .map_err(|error| RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: self.registry_name().to_string(),
                message: format!("invalid Last-Modified for {version}: {error}"),
            })?
            .with_timezone(&Utc);
        Ok(Some(date))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn serve_responses(
        responses: Vec<(&'static str, &'static str, &'static str)>,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            for (request_start, status_and_headers, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let read = stream.read(&mut request).unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..read]).starts_with(request_start),
                    "unexpected request: {}",
                    String::from_utf8_lossy(&request[..read])
                );
                let response = format!(
                    "HTTP/1.1 {status_and_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn metadata_contains_versions_missing_from_search_index() {
        let xml = r#"<metadata><groupId>com.squareup.okhttp3</groupId><artifactId>okhttp</artifactId><versioning><versions><version>5.4.0</version><version>5.5.0</version></versions><lastUpdated>20260816161727</lastUpdated></versioning></metadata>"#;
        let parsed: MavenMetadata = quick_xml::de::from_str(xml).unwrap();
        assert_eq!(parsed.versioning.versions.version, ["5.4.0", "5.5.0"]);
    }

    #[test]
    fn repository_urls_are_validated() {
        let adapter = MavenCentralAdapter::new(HttpClient::new().unwrap());
        assert_eq!(
            adapter.artifact_url("com.squareup.okhttp3:okhttp").unwrap(),
            "https://repo1.maven.org/maven2/com/squareup/okhttp3/okhttp"
        );
        assert_eq!(
            adapter
                .version_url("com.squareup.okhttp3:okhttp", "5.5.0")
                .unwrap(),
            "https://repo1.maven.org/maven2/com/squareup/okhttp3/okhttp/5.5.0/okhttp-5.5.0.pom"
        );
        assert!(adapter.artifact_url("../escape:okhttp").is_err());
        assert!(adapter.version_url("a:b", "../escape").is_err());
    }

    #[tokio::test]
    async fn metadata_and_pom_date_use_repository_http_responses() {
        let xml = "<metadata><groupId>example</groupId><artifactId>library</artifactId><versioning><versions><version>1.0</version><version>2.0</version></versions></versioning></metadata>";
        let (base_url, handle) = serve_responses(vec![
            (
                "GET /example/library/maven-metadata.xml ",
                "200 OK\r\nContent-Type: application/xml\r\n",
                xml,
            ),
            (
                "HEAD /example/library/2.0/library-2.0.pom ",
                "200 OK\r\nLast-Modified: Sun, 16 Aug 2026 16:12:45 GMT\r\n",
                "",
            ),
        ]);
        let mut adapter = MavenCentralAdapter::new(HttpClient::new().unwrap());
        adapter.base_url = base_url;
        let versions = adapter.fetch_versions("example:library").await.unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[1].version, "2.0");
        let date = adapter
            .fetch_release_date("example:library", "2.0")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(date.to_rfc3339(), "2026-08-16T16:12:45+00:00");
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn not_found_pom_does_not_use_its_last_modified_header() {
        let (base_url, handle) = serve_responses(vec![(
            "HEAD /example/library/2.0/library-2.0.pom ",
            "404 Not Found\r\nLast-Modified: Sat, 11 Oct 2025 00:00:00 GMT\r\n",
            "",
        )]);
        let mut adapter = MavenCentralAdapter::new(HttpClient::new().unwrap());
        adapter.base_url = base_url;
        assert!(
            adapter
                .fetch_release_date("example:library", "2.0")
                .await
                .is_err()
        );
        handle.join().unwrap();
    }
}
