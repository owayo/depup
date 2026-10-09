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
use std::collections::HashMap;
use std::path::Path;
use tokio::sync::Mutex;

const MAVEN_CENTRAL_URL: &str = "https://repo1.maven.org/maven2";
const GOOGLE_MAVEN_URL: &str = "https://dl.google.com/dl/android/maven2";
const GRADLE_PLUGIN_PORTAL_URL: &str = "https://plugins.gradle.org/m2";

pub struct MavenCentralAdapter {
    client: HttpClient,
    base_url: String,
    fallback_urls: Vec<String>,
    plugin_urls: Vec<String>,
    package_sources: Mutex<HashMap<String, String>>,
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
    fn repository_label(base_url: &str) -> &'static str {
        match base_url {
            MAVEN_CENTRAL_URL => "Maven Central",
            GOOGLE_MAVEN_URL => "Google Maven",
            GRADLE_PLUGIN_PORTAL_URL => "Gradle Plugin Portal",
            _ => "Maven repository",
        }
    }

    pub fn new(client: HttpClient) -> Self {
        Self {
            client,
            base_url: MAVEN_CENTRAL_URL.to_string(),
            fallback_urls: vec![
                GOOGLE_MAVEN_URL.to_string(),
                GRADLE_PLUGIN_PORTAL_URL.to_string(),
            ],
            plugin_urls: vec![
                MAVEN_CENTRAL_URL.to_string(),
                GOOGLE_MAVEN_URL.to_string(),
                GRADLE_PLUGIN_PORTAL_URL.to_string(),
            ],
            package_sources: Mutex::new(HashMap::new()),
        }
    }

    fn repository_urls(content: &str) -> Vec<String> {
        let mut urls = Vec::new();
        let pattern = regex::Regex::new(r"\b(google|mavenCentral|gradlePluginPortal)\s*\(\s*\)")
            .expect("static Gradle repository pattern");
        for capture in pattern.captures_iter(content) {
            let url = match &capture[1] {
                "google" => GOOGLE_MAVEN_URL,
                "mavenCentral" => MAVEN_CENTRAL_URL,
                _ => GRADLE_PLUGIN_PORTAL_URL,
            };
            if !urls.iter().any(|known| known == url) {
                urls.push(url.to_string());
            }
        }
        urls
    }

    fn settings_block<'a>(content: &'a str, name: &str) -> Option<&'a str> {
        let pattern = regex::Regex::new(&format!(r"\b{}\s*\{{", regex::escape(name))).ok()?;
        let opening = pattern.find(content)?.end() - 1;
        let mut depth = 0_u32;
        let mut quote = None;
        let mut escaped = false;
        for (offset, ch) in content[opening..].char_indices() {
            if let Some(quoted) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == quoted {
                    quote = None;
                }
                continue;
            }
            match ch {
                '\'' | '"' => quote = Some(ch),
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&content[opening + 1..opening + offset]);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Respect built-in repository declarations in the nearest Gradle settings file.
    /// In projects without one, try the public repositories in their usual order.
    pub fn for_manifest(client: HttpClient, manifest: &Path) -> Self {
        let mut adapter = Self::new(client);
        let settings = manifest.ancestors().find_map(|directory| {
            ["settings.gradle", "settings.gradle.kts"]
                .into_iter()
                .map(|name| directory.join(name))
                .find(|path| path.is_file())
        });
        let Some(settings) = settings else {
            return adapter;
        };
        let Ok(content) = std::fs::read_to_string(settings) else {
            return adapter;
        };
        let stripped = crate::manifest::strip_gradle_comments(&content);
        let global = Self::repository_urls(&stripped);
        let scoped_urls = |scope| {
            Self::settings_block(&stripped, scope)
                .and_then(|block| Self::settings_block(block, "repositories"))
                .map(Self::repository_urls)
                .filter(|urls| !urls.is_empty())
        };
        let urls = scoped_urls("dependencyResolutionManagement").unwrap_or_else(|| global.clone());
        if let Some(plugins) = scoped_urls("pluginManagement") {
            adapter.plugin_urls = plugins;
        } else if !global.is_empty() {
            adapter.plugin_urls = global;
        }
        if let Some(first) = urls.first() {
            adapter.base_url = first.clone();
            adapter.fallback_urls = urls.into_iter().skip(1).collect();
        }
        adapter
    }

    fn artifact_url_at(&self, base_url: &str, package: &str) -> Result<String, RegistryError> {
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
            base_url,
            group.replace('.', "/"),
            artifact
        ))
    }

    #[cfg(test)]
    fn artifact_url(&self, package: &str) -> Result<String, RegistryError> {
        self.artifact_url_at(&self.base_url, package)
    }

    fn invalid_package(&self, package: &str) -> RegistryError {
        RegistryError::InvalidPackageName {
            name: package.to_string(),
            registry: self.registry_name().to_string(),
            reason: "expected a safe groupId:artifactId coordinate".to_string(),
        }
    }

    fn version_url_at(
        &self,
        base_url: &str,
        package: &str,
        version: &str,
    ) -> Result<String, RegistryError> {
        let artifact_url = self.artifact_url_at(base_url, package)?;
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

    #[cfg(test)]
    fn version_url(&self, package: &str, version: &str) -> Result<String, RegistryError> {
        self.version_url_at(&self.base_url, package, version)
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

    fn cache_scope(&self) -> String {
        let libraries = std::iter::once(&self.base_url)
            .chain(self.fallback_urls.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("|");
        format!("{libraries}||{}", self.plugin_urls.join("|"))
    }

    fn release_dates_deferred(&self) -> bool {
        true
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        if package == crate::manifest::GRADLE_WRAPPER_PACKAGE {
            return super::gradle_versions::fetch_versions(&self.client).await;
        }
        let mut selected = None;
        let repositories: Vec<&String> = if package.ends_with(".gradle.plugin") {
            self.plugin_urls.iter().collect()
        } else {
            std::iter::once(&self.base_url)
                .chain(self.fallback_urls.iter())
                .collect()
        };
        for base_url in repositories {
            let url = format!(
                "{}/maven-metadata.xml",
                self.artifact_url_at(base_url, package)?
            );
            match self
                .client
                .get_text(&url, package, Self::repository_label(base_url))
                .await
            {
                Ok(xml) => {
                    selected = Some((base_url.clone(), xml));
                    break;
                }
                Err(RegistryError::PackageNotFound { .. }) => continue,
                Err(error) => return Err(error),
            }
        }
        let (base_url, xml) = selected.ok_or_else(|| RegistryError::PackageNotFound {
            package: package.to_string(),
            registry: "configured Maven repositories".to_string(),
        })?;
        let metadata: MavenMetadata =
            quick_xml::de::from_str(&xml).map_err(|error| RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: Self::repository_label(&base_url).to_string(),
                message: format!("Maven metadata XML: {error}"),
            })?;
        let (group, artifact) = package.split_once(':').expect("validated coordinate");
        if metadata.group_id != group || metadata.artifact_id != artifact {
            return Err(RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: Self::repository_label(&base_url).to_string(),
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
        self.package_sources
            .lock()
            .await
            .insert(package.to_string(), base_url);
        Ok(versions)
    }

    async fn fetch_release_date(
        &self,
        package: &str,
        version: &str,
    ) -> Result<Option<DateTime<Utc>>, RegistryError> {
        if !self.package_sources.lock().await.contains_key(package) {
            self.fetch_versions(package).await?;
        }
        let base_url = self
            .package_sources
            .lock()
            .await
            .get(package)
            .cloned()
            .unwrap_or_else(|| self.base_url.clone());
        let url = self.version_url_at(&base_url, package, version)?;
        let registry = Self::repository_label(&base_url);
        let response = self
            .client
            .get_response_with_retry(|| self.client.inner().head(&url), package, registry)
            .await?;
        if let Some(error) =
            crate::registry::client::map_status_error(response.status(), package, registry)
        {
            return Err(error);
        }
        let header = response.headers().get(LAST_MODIFIED).ok_or_else(|| {
            RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: registry.to_string(),
                message: format!("POM for {version} has no Last-Modified header"),
            }
        })?;
        let date = DateTime::parse_from_rfc2822(header.to_str().unwrap_or(""))
            .map_err(|error| RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: registry.to_string(),
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

    #[test]
    fn settings_repository_order_is_used_for_nested_catalog() {
        let root = tempfile::TempDir::new().unwrap();
        std::fs::write(
            root.path().join("settings.gradle.kts"),
            "// mavenCentral()\npluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }\ndependencyResolutionManagement { repositories { mavenCentral(); google() } }",
        )
        .unwrap();
        let manifest = root.path().join("gradle/libs.versions.toml");
        let adapter = MavenCentralAdapter::for_manifest(HttpClient::new().unwrap(), &manifest);
        assert_eq!(adapter.base_url, MAVEN_CENTRAL_URL);
        assert_eq!(adapter.fallback_urls, vec![GOOGLE_MAVEN_URL]);
        assert_eq!(
            adapter.plugin_urls,
            vec![
                GOOGLE_MAVEN_URL,
                MAVEN_CENTRAL_URL,
                GRADLE_PLUGIN_PORTAL_URL
            ]
        );
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
    async fn missing_central_artifact_uses_google_repository_for_metadata_and_date() {
        let (central, central_handle) = serve_responses(vec![(
            "GET /example/library/maven-metadata.xml ",
            "404 Not Found\r\n",
            "",
        )]);
        let xml = "<metadata><groupId>example</groupId><artifactId>library</artifactId><versioning><versions><version>2.0</version></versions></versioning></metadata>";
        let (google, google_handle) = serve_responses(vec![
            (
                "GET /example/library/maven-metadata.xml ",
                "200 OK\r\nContent-Type: application/xml\r\n",
                xml,
            ),
            (
                "HEAD /example/library/2.0/library-2.0.pom ",
                "200 OK\r\nLast-Modified: Wed, 23 Sep 2026 00:00:00 GMT\r\n",
                "",
            ),
        ]);
        let mut adapter = MavenCentralAdapter::new(HttpClient::new().unwrap());
        adapter.base_url = central;
        adapter.fallback_urls = vec![google];
        let versions = adapter.fetch_versions("example:library").await.unwrap();
        assert_eq!(versions.len(), 1);
        let date = adapter
            .fetch_release_date("example:library", "2.0")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(date.to_rfc3339(), "2026-09-23T00:00:00+00:00");
        central_handle.join().unwrap();
        google_handle.join().unwrap();
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
        adapter
            .package_sources
            .lock()
            .await
            .insert("example:library".to_string(), adapter.base_url.clone());
        assert!(
            adapter
                .fetch_release_date("example:library", "2.0")
                .await
                .is_err()
        );
        handle.join().unwrap();
    }
}
