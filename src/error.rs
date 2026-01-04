//! Application error types using thiserror
//!
//! Error hierarchy:
//! - ManifestError: Issues with manifest file parsing
//! - RegistryError: Issues with package registry communication
//! - ConfigError: Issues with CLI configuration
//! - IoError: File system operation failures

use std::path::PathBuf;
use thiserror::Error;

use crate::domain::Language;

/// Application-level error type
#[derive(Error, Debug)]
pub enum AppError {
    /// Manifest file related errors
    #[error(transparent)]
    Manifest(#[from] ManifestError),

    /// Package registry related errors
    #[error(transparent)]
    Registry(#[from] RegistryError),

    /// Configuration related errors
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// IO related errors
    #[error(transparent)]
    Io(#[from] IoError),
}

/// Errors related to manifest file operations
#[derive(Error, Debug)]
pub enum ManifestError {
    /// Manifest file not found
    #[error("manifest file not found: {path}")]
    NotFound { path: PathBuf },

    /// Failed to read manifest file
    #[error("failed to read manifest file {path}: {source}")]
    ReadError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to write manifest file
    #[error("failed to write manifest file {path}: {source}")]
    WriteError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// JSON parsing error (for package.json)
    #[error("failed to parse JSON in {path}: {message}")]
    JsonParseError { path: PathBuf, message: String },

    /// TOML parsing error (for pyproject.toml, Cargo.toml)
    #[error("failed to parse TOML in {path}: {message}")]
    TomlParseError { path: PathBuf, message: String },

    /// go.mod parsing error
    #[error("failed to parse go.mod in {path}: {message}")]
    GoModParseError { path: PathBuf, message: String },

    /// Invalid version specification
    #[error("invalid version specification '{spec}' in {path}: {message}")]
    InvalidVersionSpec {
        path: PathBuf,
        spec: String,
        message: String,
    },

    /// Unsupported manifest format
    #[error("unsupported manifest format: {path}")]
    UnsupportedFormat { path: PathBuf },
}

/// Errors related to package registry communication
#[derive(Error, Debug)]
pub enum RegistryError {
    /// Package not found in registry
    #[error("package '{package}' not found in {registry} registry")]
    PackageNotFound { package: String, registry: String },

    /// Network request failed
    #[error("failed to fetch package '{package}' from {registry}: {message}")]
    NetworkError {
        package: String,
        registry: String,
        message: String,
    },

    /// Rate limit exceeded
    #[error("rate limit exceeded for {registry} registry")]
    RateLimitExceeded { registry: String },

    /// Invalid response from registry
    #[error("invalid response from {registry} for '{package}': {message}")]
    InvalidResponse {
        package: String,
        registry: String,
        message: String,
    },

    /// Timeout
    #[error("timeout while fetching '{package}' from {registry}")]
    Timeout { package: String, registry: String },

    /// Authentication error
    #[error("authentication failed for {registry}: {message}")]
    AuthenticationError { registry: String, message: String },
}

/// Errors related to configuration
#[derive(Error, Debug)]
pub enum ConfigError {
    /// Invalid duration format
    #[error("invalid duration format '{value}': expected format like '2w', '10d', '1m'")]
    InvalidDuration { value: String },

    /// Invalid language filter
    #[error("invalid language filter '{value}': expected 'node', 'python', 'rust', or 'go'")]
    InvalidLanguageFilter { value: String },

    /// Invalid path
    #[error("invalid path '{path}': {message}")]
    InvalidPath { path: PathBuf, message: String },

    /// Conflicting options
    #[error("conflicting options: {message}")]
    ConflictingOptions { message: String },
}

/// Errors related to IO operations
#[derive(Error, Debug)]
pub enum IoError {
    /// Directory not found
    #[error("directory not found: {path}")]
    DirectoryNotFound { path: PathBuf },

    /// Permission denied
    #[error("permission denied: {path}")]
    PermissionDenied { path: PathBuf },

    /// Generic IO error
    #[error("IO error at {path}: {source}")]
    Generic {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl ManifestError {
    /// Creates a new NotFound error
    pub fn not_found(path: impl Into<PathBuf>) -> Self {
        ManifestError::NotFound { path: path.into() }
    }

    /// Creates a new ReadError
    pub fn read_error(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        ManifestError::ReadError {
            path: path.into(),
            source,
        }
    }

    /// Creates a new WriteError
    pub fn write_error(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        ManifestError::WriteError {
            path: path.into(),
            source,
        }
    }

    /// Creates a new JsonParseError
    pub fn json_parse_error(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        ManifestError::JsonParseError {
            path: path.into(),
            message: message.into(),
        }
    }

    /// Creates a new TomlParseError
    pub fn toml_parse_error(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        ManifestError::TomlParseError {
            path: path.into(),
            message: message.into(),
        }
    }

    /// Creates a new InvalidVersionSpec error
    pub fn invalid_version_spec(
        path: impl Into<PathBuf>,
        spec: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        ManifestError::InvalidVersionSpec {
            path: path.into(),
            spec: spec.into(),
            message: message.into(),
        }
    }
}

impl RegistryError {
    /// Creates a new PackageNotFound error
    pub fn package_not_found(package: impl Into<String>, registry: impl Into<String>) -> Self {
        RegistryError::PackageNotFound {
            package: package.into(),
            registry: registry.into(),
        }
    }

    /// Creates a new NetworkError
    pub fn network_error(
        package: impl Into<String>,
        registry: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        RegistryError::NetworkError {
            package: package.into(),
            registry: registry.into(),
            message: message.into(),
        }
    }

    /// Creates a new RateLimitExceeded error
    pub fn rate_limit_exceeded(registry: impl Into<String>) -> Self {
        RegistryError::RateLimitExceeded {
            registry: registry.into(),
        }
    }

    /// Creates a new Timeout error
    pub fn timeout(package: impl Into<String>, registry: impl Into<String>) -> Self {
        RegistryError::Timeout {
            package: package.into(),
            registry: registry.into(),
        }
    }

    /// Returns the registry name for this language
    pub fn registry_name(language: Language) -> &'static str {
        match language {
            Language::Node => "npm",
            Language::Python => "PyPI",
            Language::Rust => "crates.io",
            Language::Go => "Go Proxy",
            Language::Ruby => "RubyGems",
            Language::Php => "Packagist",
        }
    }
}

impl IoError {
    /// Creates a new DirectoryNotFound error
    pub fn directory_not_found(path: impl Into<PathBuf>) -> Self {
        IoError::DirectoryNotFound { path: path.into() }
    }

    /// Creates a new PermissionDenied error
    pub fn permission_denied(path: impl Into<PathBuf>) -> Self {
        IoError::PermissionDenied { path: path.into() }
    }

    /// Creates a new Generic IO error
    pub fn generic(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        IoError::Generic {
            path: path.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_error_not_found() {
        let err = ManifestError::not_found("/path/to/package.json");
        let msg = format!("{}", err);
        assert!(msg.contains("manifest file not found"));
        assert!(msg.contains("package.json"));
    }

    #[test]
    fn test_manifest_error_json_parse() {
        let err = ManifestError::json_parse_error("/path/to/package.json", "unexpected token");
        let msg = format!("{}", err);
        assert!(msg.contains("failed to parse JSON"));
        assert!(msg.contains("unexpected token"));
    }

    #[test]
    fn test_manifest_error_toml_parse() {
        let err = ManifestError::toml_parse_error("/path/to/Cargo.toml", "invalid key");
        let msg = format!("{}", err);
        assert!(msg.contains("failed to parse TOML"));
        assert!(msg.contains("invalid key"));
    }

    #[test]
    fn test_manifest_error_invalid_version_spec() {
        let err = ManifestError::invalid_version_spec(
            "/path/to/package.json",
            ">>1.0",
            "invalid operator",
        );
        let msg = format!("{}", err);
        assert!(msg.contains("invalid version specification"));
        assert!(msg.contains(">>1.0"));
    }

    #[test]
    fn test_registry_error_package_not_found() {
        let err = RegistryError::package_not_found("nonexistent-package", "npm");
        let msg = format!("{}", err);
        assert!(msg.contains("package 'nonexistent-package' not found"));
        assert!(msg.contains("npm"));
    }

    #[test]
    fn test_registry_error_network() {
        let err = RegistryError::network_error("lodash", "npm", "connection refused");
        let msg = format!("{}", err);
        assert!(msg.contains("failed to fetch"));
        assert!(msg.contains("connection refused"));
    }

    #[test]
    fn test_registry_error_rate_limit() {
        let err = RegistryError::rate_limit_exceeded("crates.io");
        let msg = format!("{}", err);
        assert!(msg.contains("rate limit exceeded"));
        assert!(msg.contains("crates.io"));
    }

    #[test]
    fn test_registry_error_timeout() {
        let err = RegistryError::timeout("serde", "crates.io");
        let msg = format!("{}", err);
        assert!(msg.contains("timeout"));
        assert!(msg.contains("serde"));
    }

    #[test]
    fn test_registry_name() {
        assert_eq!(RegistryError::registry_name(Language::Node), "npm");
        assert_eq!(RegistryError::registry_name(Language::Python), "PyPI");
        assert_eq!(RegistryError::registry_name(Language::Rust), "crates.io");
        assert_eq!(RegistryError::registry_name(Language::Go), "Go Proxy");
    }

    #[test]
    fn test_config_error_invalid_duration() {
        let err = ConfigError::InvalidDuration {
            value: "abc".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("invalid duration format"));
        assert!(msg.contains("abc"));
    }

    #[test]
    fn test_config_error_conflicting_options() {
        let err = ConfigError::ConflictingOptions {
            message: "--quiet and --verbose cannot be used together".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("conflicting options"));
    }

    #[test]
    fn test_io_error_directory_not_found() {
        let err = IoError::directory_not_found("/path/to/missing");
        let msg = format!("{}", err);
        assert!(msg.contains("directory not found"));
    }

    #[test]
    fn test_io_error_permission_denied() {
        let err = IoError::permission_denied("/path/to/protected");
        let msg = format!("{}", err);
        assert!(msg.contains("permission denied"));
    }

    #[test]
    fn test_app_error_from_manifest_error() {
        let manifest_err = ManifestError::not_found("/path");
        let app_err: AppError = manifest_err.into();
        let msg = format!("{}", app_err);
        assert!(msg.contains("manifest file not found"));
    }

    #[test]
    fn test_app_error_from_registry_error() {
        let registry_err = RegistryError::package_not_found("pkg", "npm");
        let app_err: AppError = registry_err.into();
        let msg = format!("{}", app_err);
        assert!(msg.contains("package 'pkg' not found"));
    }

    #[test]
    fn test_app_error_from_config_error() {
        let config_err = ConfigError::InvalidDuration {
            value: "bad".to_string(),
        };
        let app_err: AppError = config_err.into();
        let msg = format!("{}", app_err);
        assert!(msg.contains("invalid duration format"));
    }

    #[test]
    fn test_app_error_from_io_error() {
        let io_err = IoError::directory_not_found("/missing");
        let app_err: AppError = io_err.into();
        let msg = format!("{}", app_err);
        assert!(msg.contains("directory not found"));
    }

    #[test]
    fn test_error_debug_trait() {
        let err = ManifestError::not_found("/test");
        let debug = format!("{:?}", err);
        assert!(debug.contains("NotFound"));
    }
}
