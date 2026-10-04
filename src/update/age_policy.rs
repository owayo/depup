//! 設定された身元と、レジストリが確認した公開者・取得元を照合する age ポリシー。

use crate::domain::Language;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::VersionInfo;

/// 候補選択・同期・install・lock 監査で共有する、解決済みの age 制約。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgePolicy {
    pub min_age: Option<Duration>,
    pub evaluated_at: DateTime<Utc>,
    pub exemptions: AgeExemptions,
}

impl AgePolicy {
    pub fn cutoff(&self) -> Option<DateTime<Utc>> {
        self.min_age
            .and_then(|age| crate::domain::cutoff_from(self.evaluated_at, age))
    }

    /// install 中に時間が進んでも、候補選択時の境界を緩めない。
    pub fn install_min_age(&self) -> Option<Duration> {
        self.cutoff()
            .map(|cutoff| (Utc::now() - cutoff).to_std().unwrap_or_default())
    }

    /// 同じ install / lock を共有するマニフェストの制約をすべて守る。
    pub fn merge(&mut self, other: &Self) {
        let cutoff = match (self.cutoff(), other.cutoff()) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        self.evaluated_at = self.evaluated_at.max(other.evaluated_at);
        self.min_age =
            cutoff.map(|cutoff| (self.evaluated_at - cutoff).to_std().unwrap_or_default());
        self.exemptions.github.retain(|login| {
            other
                .exemptions
                .github
                .iter()
                .any(|other| login.eq_ignore_ascii_case(other))
        });
    }
}

/// GitHub の身元をレジストリが確認した、版ごとの公開者情報。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PublisherEvidence {
    #[default]
    Unknown,
    /// crates.io が GitHub と一致すると確認したログイン名。
    GithubUser { login: String },
    /// crates.io の Trusted Publishing が検証した GitHub リポジトリの所有者。
    GithubTrustedPublisher { owner: String },
}

impl PublisherEvidence {
    pub(crate) fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

/// age だけを免除する身元。アカウントは利用者の設定に閉じる。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgeExemptions {
    #[serde(default)]
    pub github: Vec<String>,
}

impl AgeExemptions {
    pub fn is_empty(&self) -> bool {
        self.github.is_empty()
    }

    pub(crate) fn validate(&mut self) {
        self.github.retain(|login| {
            let valid = valid_github_login(login);
            if !valid {
                eprintln!("Warning: invalid GitHub login in age_exempt.github: {login:?}");
            }
            valid
        });
        for login in &mut self.github {
            login.make_ascii_lowercase();
        }
        self.github.sort();
        self.github.dedup();
    }

    /// 全 age 判定で共有する。情報が欠ける場合は通常の cutoff を維持する。
    pub(crate) fn admits(
        &self,
        language: Language,
        package: &str,
        version: &VersionInfo,
        cutoff: DateTime<Utc>,
    ) -> bool {
        version.released_at <= cutoff || self.exemption(language, package, version).is_some()
    }

    pub(crate) fn exemption(
        &self,
        language: Language,
        package: &str,
        version: &VersionInfo,
    ) -> Option<AgeExemption> {
        let (owner, evidence) = match language {
            Language::Rust => match &version.publisher {
                PublisherEvidence::GithubUser { login } => (login.as_str(), "crates_io_publisher"),
                PublisherEvidence::GithubTrustedPublisher { owner } => {
                    (owner.as_str(), "trusted_publishing")
                }
                PublisherEvidence::Unknown => return None,
            },
            Language::Go => (
                github_owner_repo(package.strip_prefix("github.com/")?)?,
                "github_source",
            ),
            // PackageSwiftParser は GitHub の取得元を検証して owner/repo に正規化する。
            Language::Swift => (github_owner_repo(package)?, "github_source"),
            _ => return None,
        };
        if !valid_github_login(owner)
            || !self
                .github
                .iter()
                .any(|login| login.eq_ignore_ascii_case(owner))
        {
            return None;
        }
        Some(AgeExemption {
            identity: format!("github:{}", owner.to_ascii_lowercase()),
            evidence: evidence.to_string(),
        })
    }
}

/// 出力に使う、免除された身元と根拠。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgeExemption {
    pub identity: String,
    pub evidence: String,
}

pub(crate) fn valid_github_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && !login.ends_with('-')
        && !login.contains("--")
        && login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Go のサブモジュールも許可するが、所有者とリポジトリはセグメントで照合する。
fn github_owner_repo(path: &str) -> Option<&str> {
    let mut segments = path.split('/');
    let owner = segments.next()?;
    let repo = segments.next()?;
    if !valid_github_login(owner)
        || !crate::registry::is_valid_registry_id_segment(repo)
        || segments.any(|segment| !crate::registry::is_valid_registry_id_segment(segment))
    {
        return None;
    }
    Some(owner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_policy_keeps_the_oldest_cutoff_and_common_exemptions() {
        let mut policy = AgePolicy {
            min_age: Some(Duration::from_secs(14 * 86400)),
            evaluated_at: "2026-10-01T00:00:00Z".parse().unwrap(),
            exemptions: AgeExemptions {
                github: vec!["example-dev".into(), "another-dev".into()],
            },
        };
        let cutoff = policy.cutoff().unwrap();
        let other = AgePolicy {
            min_age: Some(Duration::from_secs(7 * 86400)),
            evaluated_at: "2026-10-04T00:00:00Z".parse().unwrap(),
            exemptions: AgeExemptions {
                github: vec!["EXAMPLE-DEV".into()],
            },
        };
        policy.merge(&other);
        assert_eq!(policy.cutoff(), Some(cutoff));
        assert_eq!(policy.exemptions.github, ["example-dev"]);
        policy.merge(&AgePolicy {
            min_age: None,
            exemptions: Default::default(),
            ..other
        });
        assert_eq!(policy.cutoff(), Some(cutoff));
        assert!(policy.exemptions.is_empty());
    }

    #[test]
    fn only_configured_and_verified_identities_bypass_age() {
        let exemptions = AgeExemptions {
            github: vec!["example-dev".into()],
        };
        let cutoff = Utc::now() - chrono::Duration::days(14);
        let unknown = VersionInfo::now("1.1.0");
        assert!(!exemptions.admits(Language::Rust, "library", &unknown, cutoff));
        let mut known = unknown.clone();
        known.publisher = PublisherEvidence::GithubUser {
            login: "EXAMPLE-DEV".into(),
        };
        assert!(exemptions.admits(Language::Rust, "library", &known, cutoff));
        known.publisher = PublisherEvidence::GithubUser {
            login: "another-dev".into(),
        };
        assert!(!exemptions.admits(Language::Rust, "library", &known, cutoff));
        // 別レジストリの同名アカウントは GitHub の身元にはならない。
        assert!(!exemptions.admits(Language::Node, "@example-dev/library", &known, cutoff));
        assert!(!AgeExemptions::default().admits(Language::Rust, "library", &known, cutoff));
        let old = VersionInfo::new("1.0.0", cutoff);
        assert!(exemptions.admits(Language::Rust, "library", &old, cutoff));
    }

    #[test]
    fn source_identity_matches_complete_github_segments() {
        let exemptions = AgeExemptions {
            github: vec!["example-dev".into()],
        };
        let version = VersionInfo::now("1.1.0");
        for name in [
            "github.com/example-dev/library",
            "github.com/Example-Dev/library/sub/v2",
        ] {
            assert!(
                exemptions.exemption(Language::Go, name, &version).is_some(),
                "{name}"
            );
        }
        for name in [
            "github.com/example-dev-evil/library",
            "example.com/github.com/example-dev/library",
            "github.com/example-dev/../other",
            "github.com/example-dev",
        ] {
            assert!(
                exemptions.exemption(Language::Go, name, &version).is_none(),
                "{name}"
            );
        }
        assert!(
            exemptions
                .exemption(Language::Swift, "example-dev/library", &version)
                .is_some()
        );
        assert!(
            exemptions
                .exemption(Language::Python, "example-dev/library", &version)
                .is_none()
        );
    }
}
