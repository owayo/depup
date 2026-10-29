//! 解消した脆弱性の重大度・件数を超えない範囲で、新規 Medium / Low を許容する。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
    #[default]
    Unknown,
}

impl Severity {
    fn label(value: &str) -> Self {
        match value.to_ascii_uppercase().as_str() {
            "LOW" => Self::Low,
            "MEDIUM" | "MODERATE" => Self::Medium,
            "HIGH" => Self::High,
            "CRITICAL" => Self::Critical,
            _ => Self::Unknown,
        }
    }

    /// 同一 advisory の複数の評価は、取得できた評価のうち最も厳しいものを使う。
    pub(super) fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, known) | (known, Self::Unknown) => known,
            (a, b) => a.max(b),
        }
    }

    fn medium_or_above(self) -> bool {
        matches!(self, Self::Medium | Self::High | Self::Critical)
    }
}

/// API のレコード。aliases は同一性にだけ使用し、related は含めない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advisory {
    pub id: String,
    pub aliases: Vec<String>,
    pub severity: Severity,
}

impl Advisory {
    pub(crate) fn malicious(&self) -> bool {
        std::iter::once(&self.id)
            .chain(&self.aliases)
            .any(|id| id.to_ascii_uppercase().starts_with("MAL-"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvisoryGroup {
    pub ids: Vec<String>,
    pub severity: Severity,
}

/// 許容した更新の根拠。OSV 検査で脆弱性がゼロだったこととは区別する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskTradeoff {
    pub baseline_versions: Vec<String>,
    pub candidate_versions: Vec<String>,
    pub fixed: Vec<AdvisoryGroup>,
    pub introduced: Vec<AdvisoryGroup>,
    pub retained: Vec<AdvisoryGroup>,
}

impl RiskTradeoff {
    pub fn describe_summary(&self, name: &str) -> String {
        format!(
            "{name} {} -> {}: OSV severity trade-off accepted (fixed: {}, introduced: {}, retained: {}); known vulnerabilities remain; details: --verbose",
            self.baseline_versions.join(", "),
            self.candidate_versions.join(", "),
            self.fixed.len(),
            self.introduced.len(),
            self.retained.len()
        )
    }

    pub fn describe(&self, name: &str) -> String {
        let list = |groups: &[AdvisoryGroup]| {
            groups
                .iter()
                .map(|group| format!("{} [{:?}]", group.ids.join("/"), group.severity))
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "{name} {} -> {}: OSV severity trade-off accepted; fixed: {}; introduced: {}; retained: {} (known vulnerabilities remain)",
            self.baseline_versions.join(", "),
            self.candidate_versions.join(", "),
            list(&self.fixed),
            if self.introduced.is_empty() {
                "none".into()
            } else {
                list(&self.introduced)
            },
            if self.retained.is_empty() {
                "none".into()
            } else {
                list(&self.retained)
            },
        )
    }
}

#[derive(Debug)]
pub(crate) enum Assessment {
    Safe,
    Tradeoff(RiskTradeoff),
    Rejected {
        advisories: Vec<Advisory>,
        baseline_error: Option<String>,
    },
}

struct Group {
    ids: BTreeSet<String>,
    severity: Severity,
    before: bool,
    after: bool,
}

/// 重大度ごとの件数を使い、新規 Medium を Low の解消で相殺しない。
pub(crate) fn tradeoff(before: &[Advisory], after: &[Advisory]) -> Option<RiskTradeoff> {
    if after
        .iter()
        .any(|advisory| advisory.malicious() || advisory.id.is_empty())
    {
        return None;
    }
    let mut groups: Vec<Group> = Vec::new();
    for (records, is_before) in [(before, true), (after, false)] {
        for advisory in records {
            if advisory.id.is_empty() {
                return None;
            }
            let mut group = Group {
                ids: std::iter::once(advisory.id.clone())
                    .chain(advisory.aliases.iter().filter(|id| !id.is_empty()).cloned())
                    .collect(),
                severity: advisory.severity,
                before: is_before,
                after: !is_before,
            };
            // aliases は対称・推移的。後から双方をつなぐレコードが来る場合も統合する。
            let mut index = 0;
            while index < groups.len() {
                if !group.ids.is_disjoint(&groups[index].ids) {
                    let other = groups.remove(index);
                    group.ids.extend(other.ids);
                    group.severity = group.severity.combine(other.severity);
                    group.before |= other.before;
                    group.after |= other.after;
                    index = 0;
                } else {
                    index += 1;
                }
            }
            groups.push(group);
        }
    }
    let mut fixed = Vec::new();
    let mut introduced = Vec::new();
    let mut retained = Vec::new();
    groups.sort_by(|a, b| a.ids.cmp(&b.ids));
    for group in groups {
        if group.after
            && group
                .ids
                .iter()
                .any(|id| id.to_ascii_uppercase().starts_with("MAL-"))
        {
            return None;
        }
        let entry = AdvisoryGroup {
            ids: group.ids.into_iter().collect(),
            severity: group.severity,
        };
        match (group.before, group.after) {
            (true, false) => fixed.push(entry),
            (false, true) => introduced.push(entry),
            (true, true) => retained.push(entry),
            _ => unreachable!(),
        }
    }
    let medium_budget = fixed
        .iter()
        .filter(|group| group.severity.medium_or_above())
        .count();
    let total_budget = fixed
        .iter()
        .filter(|group| group.severity != Severity::Unknown)
        .count();
    (total_budget > 0
        && introduced.len() <= total_budget
        && introduced
            .iter()
            .filter(|group| group.severity == Severity::Medium)
            .count()
            <= medium_budget
        && introduced
            .iter()
            .all(|group| matches!(group.severity, Severity::Low | Severity::Medium)))
    .then_some(RiskTradeoff {
        baseline_versions: Vec::new(),
        candidate_versions: Vec::new(),
        fixed,
        introduced,
        retained,
    })
}

#[derive(Debug, Deserialize)]
pub(super) struct RawSeverity {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) score: String,
}

pub(super) fn severity(label: Option<&str>, ratings: &[RawSeverity]) -> Severity {
    ratings.iter().fold(
        label.map_or(Severity::Unknown, Severity::label),
        |rating, raw| {
            let score = if raw.kind == "CVSS_V3" {
                cvss3(&raw.score)
            } else {
                None
            };
            rating.combine(score.map_or(Severity::Unknown, |score| {
                if score >= 9.0 {
                    Severity::Critical
                } else if score >= 7.0 {
                    Severity::High
                } else if score >= 4.0 {
                    Severity::Medium
                } else if score > 0.0 {
                    Severity::Low
                } else {
                    Severity::Unknown
                }
            }))
        },
    )
}

/// CVSS 3.0/3.1 の基本値。未対応の形式・欠落・重複は Unknown として例外に使わない。
fn cvss3(vector: &str) -> Option<f64> {
    let mut fields = vector.split('/');
    if !matches!(fields.next()?, "CVSS:3.0" | "CVSS:3.1") {
        return None;
    }
    let mut metrics = BTreeMap::new();
    for field in fields {
        let (name, value) = field.split_once(':')?;
        if metrics.insert(name, value).is_some() {
            return None;
        }
    }
    let weight = |key, options: &[(&str, f64)]| -> Option<f64> {
        let value = metrics.get(key)?;
        options
            .iter()
            .find_map(|(name, weight)| (*name == *value).then_some(*weight))
    };
    let changed = match *metrics.get("S")? {
        "C" => true,
        "U" => false,
        _ => return None,
    };
    let av = weight("AV", &[("N", 0.85), ("A", 0.62), ("L", 0.55), ("P", 0.2)])?;
    let ac = weight("AC", &[("L", 0.77), ("H", 0.44)])?;
    let pr = weight(
        "PR",
        &[
            ("N", 0.85),
            ("L", if changed { 0.68 } else { 0.62 }),
            ("H", if changed { 0.5 } else { 0.27 }),
        ],
    )?;
    let ui = weight("UI", &[("N", 0.85), ("R", 0.62)])?;
    let cia = [("N", 0.0), ("L", 0.22), ("H", 0.56)];
    let impact_base =
        1.0 - (1.0 - weight("C", &cia)?) * (1.0 - weight("I", &cia)?) * (1.0 - weight("A", &cia)?);
    let impact = if changed {
        7.52 * (impact_base - 0.029) - 3.25 * (impact_base - 0.02).powi(15)
    } else {
        6.42 * impact_base
    };
    if impact <= 0.0 {
        return Some(0.0);
    }
    let exploitability = 8.22 * av * ac * pr * ui;
    let base = ((impact + exploitability) * if changed { 1.08 } else { 1.0 }).min(10.0);
    Some(((base * 10.0 - 1e-10).ceil()) / 10.0)
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
