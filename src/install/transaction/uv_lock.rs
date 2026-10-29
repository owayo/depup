//! 一時的な age を解決にだけ使い、uv 自身に通常設定の lock へ戻させる。
use super::*;

#[derive(Clone)]
pub(super) struct Normalization {
    lock: PathBuf,
    pub resolve: PreparedInstall,
    pub normalize: PreparedInstall,
}

impl Normalization {
    pub fn prepare(lock: PathBuf, command: &mut PreparedInstall) -> Option<Self> {
        let cutoff = command
            .argv
            .windows(2)
            .find(|pair| pair[0] == "--exclude-newer")?[1]
            .clone();
        let mut resolve = command.clone();
        resolve.argv = vec!["uv".into(), "lock".into(), "--exclude-newer".into(), cutoff];
        let mut normalize = command.clone();
        normalize.argv = vec!["uv".into(), "lock".into()];
        // 環境を変更するのは、通常設定でも使えると確かめた lock からだけ。
        command.argv = vec!["uv".into(), "sync".into(), "--locked".into()];
        Some(Self {
            lock: identity(&lock),
            resolve,
            normalize,
        })
    }

    /// age の表現以外は、取得元・artifact・依存グラフを含めて一致させる。
    pub fn snapshot(&self, files: &BTreeMap<PathBuf, FileRecord>) -> Result<toml::Value, String> {
        let bytes = files
            .get(&self.lock)
            .and_then(|record| record.expected.bytes.as_deref())
            .ok_or("uv lock normalization requires the managed uv.lock")?;
        let text = std::str::from_utf8(bytes).map_err(|_| "uv.lock is not UTF-8")?;
        let mut value: toml::Value =
            toml::from_str(text).map_err(|_| "Cannot parse uv.lock during normalization")?;
        if let Some(options) = value.get_mut("options").and_then(toml::Value::as_table_mut) {
            options.remove("exclude-newer");
            options.remove("exclude-newer-span");
            if options.is_empty() {
                value.as_table_mut().unwrap().remove("options");
            }
        }
        Ok(value)
    }
}

pub(super) fn describe_change(before: &toml::Value, after: &toml::Value) -> String {
    let packages = |lock: &toml::Value| {
        let mut indexed: BTreeMap<String, Vec<toml::Value>> = BTreeMap::new();
        for package in lock
            .get("package")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = package.get("name").and_then(toml::Value::as_str) {
                let version = package
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .unwrap_or_default();
                indexed
                    .entry(format!("{name} {version}"))
                    .or_default()
                    .push(package.clone());
            }
        }
        indexed
    };
    let before = packages(before);
    let after = packages(after);
    let changed: BTreeSet<_> = before
        .keys()
        .chain(after.keys())
        .filter(|key| before.get(*key) != after.get(*key))
        .collect();
    if changed.is_empty() {
        "lock settings or metadata changed".into()
    } else {
        let labels: Vec<_> = changed.iter().take(5).map(|name| name.as_str()).collect();
        format!(
            "{}{}",
            labels.join(", "),
            if changed.len() > 5 { ", ..." } else { "" }
        )
    }
}
