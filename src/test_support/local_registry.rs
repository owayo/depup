//! テスト用の偽 crates.io (cargo の local-registry 形式) と、それを使う Cargo プロジェクト
//!
//! `--install --age` の lock 監査は `cargo update -p <name>@<現在の版> --precise <古い版>` を
//! 実際に起動して差し戻す。この経路を crates.io にも実行日にも依存せずに確かめるため、
//! Cargo の source replacement で crates-io を一時ディレクトリの local-registry へ差し替えた
//! プロジェクトを作り、本物の cargo に解決させる。
//!
//! 実測 (cargo 1.98.1) で分かっていること:
//! - local-registry は `<dir>/index/...` の index ファイルだけで `generate-lockfile` /
//!   `update` / `update -p X --precise V` / `pkgid` / `metadata --no-deps` が動き、
//!   `--precise` も crates.io と同じ結果になる。`.crate` ファイルは置かないので、依存の
//!   中身を読む `build` / `tree` / `metadata` (`--no-deps` なし) は失敗する。そもそもテストから
//!   `cargo build` を起動すると親の `cargo test` が持つロックと循環待ちになるので使わない
//! - `directory` source は `--precise` が何もせずに exit 0 で返るため、差し戻しの検証に使えない
//! - Cargo.lock の `source` は差し替え前の crates.io の論理 source のまま記録されるので、
//!   本番の lock 読み取り (`read_registry_entries`) をそのまま通せる

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// Cargo.lock に記録される crates.io の論理 source
const CRATES_IO_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// `.cargo/config.toml` で crates-io の差し替え先に付ける source 名
const REPLACEMENT_SOURCE: &str = "fake-crates-io";

/// 依存の種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DepKind {
    /// `[dependencies]`
    Normal,
    /// `[dev-dependencies]` (依存先の dev 依存は解決に入らない)
    Dev,
    /// `[build-dependencies]`
    Build,
}

impl DepKind {
    /// index の `kind` に書く値
    fn as_index_str(self) -> &'static str {
        match self {
            DepKind::Normal => "normal",
            DepKind::Dev => "dev",
            DepKind::Build => "build",
        }
    }
}

/// index の 1 依存
#[derive(Debug, Clone)]
pub(crate) struct IndexDep {
    /// index 上の依存名 (別名で書く場合は別名)
    pub name: String,
    /// 版要求 (`^1.0` / `=1.0.2` など)
    pub req: String,
    /// 依存の種別
    pub kind: DepKind,
    /// optional 依存か
    pub optional: bool,
    /// default feature を有効にするか
    pub default_features: bool,
    /// 有効にする feature
    pub features: Vec<String>,
    /// プラットフォーム限定の依存なら、その条件 (`cfg(windows)` やターゲット名)
    pub target: Option<String>,
    /// 別名で書く場合の実名
    pub package: Option<String>,
}

impl IndexDep {
    /// 通常依存 (`kind = normal`, optional なし, default_features = true)
    pub fn normal(name: &str, req: &str) -> Self {
        Self {
            name: name.to_string(),
            req: req.to_string(),
            kind: DepKind::Normal,
            optional: false,
            default_features: true,
            features: Vec::new(),
            target: None,
            package: None,
        }
    }

    /// index の `deps` 配列の 1 要素 (crates.io の index と同じキー)
    fn to_index_json(&self) -> serde_json::Value {
        let mut dep = serde_json::json!({
            "name": self.name,
            "req": self.req,
            "features": self.features,
            "optional": self.optional,
            "default_features": self.default_features,
            "target": self.target,
            "kind": self.kind.as_index_str(),
        });
        // crates.io の index は別名で書いた依存にだけ `package` を持つ
        if let Some(package) = &self.package {
            dep["package"] = serde_json::Value::from(package.as_str());
        }
        dep
    }
}

/// テスト用の偽 crates.io (cargo の local-registry 形式)
///
/// drop すると一時ディレクトリごと消える。`TestProject` はこのパスを参照するだけなので、
/// プロジェクトで cargo を動かし終えるまでレジストリを生かしておくこと。
pub(crate) struct LocalRegistry {
    /// レジストリのルート (直下に `index/` を持つ)
    dir: TempDir,
}

impl LocalRegistry {
    /// 版を 1 つも持たないレジストリを作る
    pub fn new() -> Self {
        let dir = TempDir::new().expect("偽 crates.io の一時ディレクトリを作れない");
        let index = dir.path().join("index");
        fs::create_dir_all(&index)
            .unwrap_or_else(|e| panic!("{} を作れない: {e}", index.display()));
        Self { dir }
    }

    /// レジストリのルート (`.cargo/config.toml` の `local-registry` に書くパス)
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// 版を 1 つ追加する (`deps` は (依存名, 版要求) の通常依存)。同じ crate の版は呼んだ順に index へ追記する
    pub fn publish(&self, name: &str, version: &str, deps: &[(&str, &str)]) -> &Self {
        let deps: Vec<IndexDep> = deps
            .iter()
            .map(|(dep, req)| IndexDep::normal(dep, req))
            .collect();
        self.publish_with(name, version, &deps)
    }

    /// 版を 1 つ追加する (依存の種別・別名・target なども指定できる版)。同じ crate の版は呼んだ順に index へ追記する
    pub fn publish_with(&self, name: &str, version: &str, deps: &[IndexDep]) -> &Self {
        let path = self.path().join("index").join(index_relative_path(name));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("{} を作れない: {e}", parent.display()));
        }
        let line = serde_json::json!({
            "name": name,
            "vers": version,
            "deps": deps.iter().map(IndexDep::to_index_json).collect::<Vec<_>>(),
            "cksum": fake_checksum(name, version),
            "features": {},
            "yanked": false,
        });
        // index は 1 行 1 版。追記なので、同じ crate の版は呼んだ順に並ぶ
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("{} を開けない: {e}", path.display()));
        writeln!(file, "{line}")
            .unwrap_or_else(|e| panic!("{} へ書き込めない: {e}", path.display()));
        self
    }

    /// 公開済みの版を yank する。既存の lock からの更新と `--precise` の再採用を検証するため。
    pub fn yank(&self, name: &str, version: &str) -> &Self {
        let path = self.path().join("index").join(index_relative_path(name));
        let mut entries: Vec<serde_json::Value> = fs::read_to_string(&path)
            .expect("公開済みクレートの index を読めない")
            .lines()
            .map(|line| serde_json::from_str(line).expect("index の版情報を解析できない"))
            .collect();
        entries
            .iter_mut()
            .find(|entry| entry["vers"] == version)
            .expect("yank 対象の版が index にない")["yanked"] = true.into();
        let content = entries
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, format!("{content}\n")).expect("yank 後の index を書けない");
        self
    }
}

/// crates.io と同じ index のパス規則 (名前を小文字にしてから、長さで振り分ける)
///
/// 1 文字は `1/<n>`、2 文字は `2/<n>`、3 文字は `3/<先頭1文字>/<n>`、
/// 4 文字以上は `<先頭2文字>/<3-4文字目>/<n>`。
fn index_relative_path(name: &str) -> PathBuf {
    // バイト位置で切り分けるので ASCII に限る (crate 名はもともと ASCII しか使えない)
    assert!(
        !name.is_empty() && name.is_ascii(),
        "crate 名として使えない: {name:?}"
    );
    let lower = name.to_ascii_lowercase();
    let name = lower.as_str();
    let dirs = match name.len() {
        1 => vec!["1"],
        2 => vec!["2"],
        3 => vec!["3", &name[..1]],
        _ => vec![&name[..2], &name[2..4]],
    };
    dirs.into_iter().chain([name]).collect()
}

/// index の `cksum` に入れる 64 桁の 16 進
///
/// `.crate` を置かないので照合されることはないが、Cargo.lock の `checksum` に
/// そのまま記録される。実行のたびに lock の中身が変わらないよう名前と版だけから決め、
/// 依存を増やさずに作るため FNV-1a (64 bit) を初期値を変えて 4 回回して連結する。
fn fake_checksum(name: &str, version: &str) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let input = format!("{name}-{version}");
    (0..4u64)
        .map(|lane| {
            let hash = input.bytes().fold(FNV_OFFSET_BASIS ^ lane, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
            });
            format!("{hash:016x}")
        })
        .collect()
}

/// crates-io を `registry_dir` の local-registry へ差し替える `.cargo/config.toml` の本文
fn source_replacement_config(registry_dir: &str) -> String {
    // パスは TOML の文字列値として書き出す。Windows の一時ディレクトリ
    // (`C:\Users\...\Temp\...`) を `"..."` へそのまま埋め込むと、`\U` などが
    // エスケープとして解釈されて壊れる
    let registry_dir = toml::Value::String(registry_dir.to_string());
    // `[resolver]`: 偽 index の版は rust-version を持たないので、MSRV を考慮した解決は
    // 結果を変えない。既定 (edition 2024 の fallback) のままだと解決のたびに
    // `rustc -vV` を起動し、テストが PATH 上の rustc とその版に左右されるので切る
    format!(
        "[source.crates-io]\n\
         replace-with = \"{REPLACEMENT_SOURCE}\"\n\
         \n\
         [source.{REPLACEMENT_SOURCE}]\n\
         local-registry = {registry_dir}\n\
         \n\
         [resolver]\n\
         incompatible-rust-versions = \"allow\"\n"
    )
}

/// 偽 crates.io を置き換え先に使う Cargo プロジェクト (一時ディレクトリ)
///
/// 一時ディレクトリの直下に Cargo プロジェクト (`project/`) と、このプロジェクト専用の
/// CARGO_HOME (`cargo-home/`) を並べて置く。cargo はパッケージキャッシュのロックや
/// キャッシュの記録を CARGO_HOME に作るため、利用者の `~/.cargo` のままでは
/// 親の `cargo test` や手元の別の cargo とロックを取り合い、利用者の設定にも結果が左右される。
pub(crate) struct TestProject {
    /// `project/` と `cargo-home/` を置く一時ディレクトリ (drop で消える)
    root: TempDir,
    /// Cargo プロジェクトのディレクトリ (`root/project`)
    dir: PathBuf,
}

impl TestProject {
    /// `cargo_toml` を Cargo.toml に書き、空の src/lib.rs を作り、
    /// `.cargo/config.toml` で crates-io を `registry` (local-registry) に置き換える
    pub fn new(registry: &LocalRegistry, cargo_toml: &str) -> Self {
        let root = TempDir::new().expect("テスト用プロジェクトの一時ディレクトリを作れない");
        let dir = root.path().join("project");
        let project = Self { root, dir };
        let cargo_home = project.cargo_home();
        fs::create_dir_all(&cargo_home)
            .unwrap_or_else(|e| panic!("{} を作れない: {e}", cargo_home.display()));
        project.write("Cargo.toml", cargo_toml);
        project.write("src/lib.rs", "");
        let registry_dir = registry
            .path()
            .to_str()
            .expect("偽 crates.io のパスが UTF-8 でない");
        project.write(
            ".cargo/config.toml",
            &source_replacement_config(registry_dir),
        );
        project
    }

    /// Cargo プロジェクトのディレクトリ
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// プロジェクト内の相対パスへ書く (親ディレクトリは作る)
    pub fn write(&self, rel: &str, content: &str) {
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("{} を作れない: {e}", parent.display()));
        }
        fs::write(&path, content).unwrap_or_else(|e| panic!("{} へ書けない: {e}", path.display()));
    }

    /// プロジェクト内の相対パスを読む
    pub fn read(&self, rel: &str) -> String {
        let path = self.dir.join(rel);
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
    }

    /// テストで cargo を起動するときのプログラム。
    /// 環境変数 `CARGO` (cargo test が設定する実行中の cargo) があればそれ、無ければ "cargo"
    pub fn cargo_program() -> OsString {
        std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
    }

    /// cargo 起動時に足す環境変数
    ///
    /// - `CARGO_HOME`: このプロジェクト専用の一時ディレクトリ。利用者の `~/.cargo` の設定・
    ///   キャッシュ・ロックに触れない
    /// - `CARGO_NET_OFFLINE=true`: ネットワークに出ない (偽 crates.io の解決に通信は要らない)
    /// - `CARGO_TERM_COLOR=never`: 取り込んだ出力に色のエスケープシーケンスを混ぜない。
    ///   CI は `CARGO_TERM_COLOR=always` を設定しており、子の cargo がそれを引き継ぐと
    ///   パイプ先でも `error` や `Downgrading` の見出しがエスケープで囲まれ、
    ///   `Downgrading solo` のような文言で照合するテストが CI でだけ落ちる
    pub fn cargo_envs(&self) -> Vec<(OsString, OsString)> {
        vec![
            (
                OsString::from("CARGO_HOME"),
                self.cargo_home().into_os_string(),
            ),
            (OsString::from("CARGO_NET_OFFLINE"), OsString::from("true")),
            (OsString::from("CARGO_TERM_COLOR"), OsString::from("never")),
        ]
    }

    /// プロジェクトのディレクトリで cargo を同期実行し、(成功したか, stdout + stderr) を返す
    pub fn cargo(&self, args: &[&str]) -> (bool, String) {
        let output = Command::new(Self::cargo_program())
            .args(args)
            .current_dir(&self.dir)
            .envs(self.cargo_envs())
            .output()
            .unwrap_or_else(|e| panic!("cargo {} を起動できない: {e}", args.join(" ")));
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        (output.status.success(), text)
    }

    /// 失敗したら出力付きで panic する版
    pub fn cargo_ok(&self, args: &[&str]) -> String {
        let (ok, output) = self.cargo(args);
        assert!(ok, "cargo {} が失敗した:\n{output}", args.join(" "));
        output
    }

    /// Cargo.lock 上の `name` の版を semver 昇順で返す。
    /// 数えるのは source が crates.io の論理 source
    /// (`registry+https://github.com/rust-lang/crates.io-index`) のものだけ
    pub fn locked_versions(&self, name: &str) -> Vec<String> {
        let content = self.read("Cargo.lock");
        let lock: toml::Table = toml::from_str(&content)
            .unwrap_or_else(|e| panic!("Cargo.lock を TOML として読めない: {e}\n{content}"));
        let packages = lock
            .get("package")
            .and_then(toml::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut versions: Vec<semver::Version> = packages
            .iter()
            .filter(|pkg| pkg.get("name").and_then(toml::Value::as_str) == Some(name))
            // path 依存 (source なし) や git 依存、別レジストリの同名 crate は数えない
            .filter(|pkg| pkg.get("source").and_then(toml::Value::as_str) == Some(CRATES_IO_SOURCE))
            .map(|pkg| {
                let version = pkg
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .unwrap_or_else(|| panic!("Cargo.lock の {name} に version が無い"));
                semver::Version::parse(version)
                    .unwrap_or_else(|e| panic!("{name} {version} を semver として読めない: {e}"))
            })
            .collect();
        versions.sort();
        versions.iter().map(ToString::to_string).collect()
    }

    /// このプロジェクト専用の CARGO_HOME (プロジェクトの外、同じ一時ディレクトリの中)
    fn cargo_home(&self) -> PathBuf {
        self.root.path().join("cargo-home")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::ffi::OsStr;

    /// `[dependencies]` だけを持つプロジェクトの Cargo.toml
    fn manifest(deps: &[(&str, &str)]) -> String {
        let mut toml = String::from(
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n",
        );
        for (name, req) in deps {
            toml.push_str(&format!("{name} = \"{req}\"\n"));
        }
        toml
    }

    /// `cargo_envs` から 1 つの値を取り出す
    fn env_value(envs: &[(OsString, OsString)], key: &str) -> Option<OsString> {
        envs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn test_index_relative_path_follows_crates_io_layout() {
        let path = |name: &str| index_relative_path(name);
        assert_eq!(path("a"), Path::new("1").join("a"));
        assert_eq!(path("ab"), Path::new("2").join("ab"));
        assert_eq!(path("abc"), Path::new("3").join("a").join("abc"));
        assert_eq!(path("solo"), Path::new("so").join("lo").join("solo"));
        assert_eq!(
            path("fam-core"),
            Path::new("fa").join("m-").join("fam-core")
        );
        // ディレクトリ名もファイル名も小文字にする
        assert_eq!(
            path("Serde_JSON"),
            Path::new("se").join("rd").join("serde_json")
        );
    }

    /// Windows の一時ディレクトリのパス (`\` 区切り) や引用符を含むパスも、
    /// TOML として読み戻すと元のパスになる (CI の Windows でも config が壊れない)
    #[test]
    fn test_source_replacement_config_keeps_windows_paths_intact() {
        for registry_dir in [
            r"C:\Users\runneradmin\AppData\Local\Temp\.tmpA1b2C3",
            r#"C:\it's "quoted"\reg"#,
            "/tmp/.tmpA1b2C3",
        ] {
            let config: toml::Table = toml::from_str(&source_replacement_config(registry_dir))
                .unwrap_or_else(|e| panic!("{registry_dir} の config を読めない: {e}"));
            let replace_with = config["source"]["crates-io"]["replace-with"]
                .as_str()
                .unwrap();
            assert_eq!(
                config["source"][replace_with]["local-registry"].as_str(),
                Some(registry_dir)
            );
            assert_eq!(
                config["resolver"]["incompatible-rust-versions"].as_str(),
                Some("allow")
            );
        }
    }

    /// 同じ crate の版は呼んだ順に 1 行ずつ追記され、依存は crates.io の index と同じキーで書かれる
    #[test]
    fn test_publish_appends_index_lines_in_call_order() {
        let registry = LocalRegistry::new();
        registry
            .publish("fam-core", "1.0.1", &[("solo", "^1.0")])
            .publish("fam-core", "1.0.0", &[])
            .publish_with(
                "fam-core",
                "1.0.2",
                &[IndexDep {
                    optional: true,
                    default_features: false,
                    features: vec!["std".to_string()],
                    target: Some("cfg(windows)".to_string()),
                    package: Some("windows-sys".to_string()),
                    ..IndexDep::normal("winsys", "^0.59")
                }],
            );

        let index = fs::read_to_string(registry.path().join("index/fa/m-/fam-core")).unwrap();
        let lines: Vec<serde_json::Value> = index
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

        let versions: Vec<&str> = lines.iter().map(|l| l["vers"].as_str().unwrap()).collect();
        assert_eq!(versions, ["1.0.1", "1.0.0", "1.0.2"]);
        assert_eq!(
            lines[0]["deps"],
            json!([{
                "name": "solo", "req": "^1.0", "features": [], "optional": false,
                "default_features": true, "target": null, "kind": "normal",
            }])
        );
        assert_eq!(
            lines[2]["deps"],
            json!([{
                "name": "winsys", "req": "^0.59", "features": ["std"], "optional": true,
                "default_features": false, "target": "cfg(windows)", "kind": "normal",
                "package": "windows-sys",
            }])
        );
        for line in &lines {
            assert_eq!(line["name"], "fam-core");
            assert_eq!(line["yanked"], false);
            let cksum = line["cksum"].as_str().unwrap();
            assert_eq!(cksum.len(), 64);
            assert!(cksum.chars().all(|c| c.is_ascii_hexdigit()));
        }
        // 版ごとに違う値になる (Cargo.lock の checksum 行が版と一緒に変わる)
        assert_ne!(lines[0]["cksum"], lines[1]["cksum"]);
    }

    /// 別名 (`package`)・build 依存・dev 依存を cargo が crates.io の index と同じ意味で読む。
    /// 別名の依存は実名で lock され、依存先の build 依存は解決に入り、dev 依存は入らない
    #[test]
    fn test_publish_with_dependency_kinds_resolve_like_crates_io() {
        let registry = LocalRegistry::new();
        registry
            .publish("solo", "1.0.0", &[])
            .publish("helper-build", "1.0.0", &[])
            .publish("helper-dev", "1.0.0", &[])
            .publish_with(
                "host",
                "1.0.0",
                &[
                    IndexDep {
                        package: Some("solo".to_string()),
                        ..IndexDep::normal("renamed", "^1.0")
                    },
                    IndexDep {
                        kind: DepKind::Build,
                        ..IndexDep::normal("helper-build", "^1.0")
                    },
                    IndexDep {
                        kind: DepKind::Dev,
                        ..IndexDep::normal("helper-dev", "^1.0")
                    },
                ],
            );
        let project = TestProject::new(&registry, &manifest(&[("host", "1.0.0")]));

        project.cargo_ok(&["generate-lockfile"]);

        assert_eq!(project.locked_versions("host"), ["1.0.0"]);
        assert_eq!(project.locked_versions("solo"), ["1.0.0"]);
        assert!(project.locked_versions("renamed").is_empty());
        assert_eq!(project.locked_versions("helper-build"), ["1.0.0"]);
        assert!(project.locked_versions("helper-dev").is_empty());
    }

    /// `=` で固定し合う相手のいない crate は、`cargo update` で上がった版を
    /// `--precise` で古い版へ戻せる
    #[test]
    fn test_precise_rolls_back_a_crate_without_lockstep_peers() {
        let registry = LocalRegistry::new();
        registry
            .publish("solo", "1.0.0", &[])
            .publish("solo", "1.0.1", &[]);
        // 古い版で lock を作ってから版要求を `^` へ戻す (`--install` 前の状態)
        let project = TestProject::new(&registry, &manifest(&[("solo", "=1.0.0")]));
        project.cargo_ok(&["generate-lockfile"]);
        assert_eq!(project.locked_versions("solo"), ["1.0.0"]);
        project.write("Cargo.toml", &manifest(&[("solo", "1.0.0")]));

        project.cargo_ok(&["update"]);
        assert_eq!(project.locked_versions("solo"), ["1.0.1"]);

        project.cargo_ok(&["update", "-p", "solo@1.0.1", "--precise", "1.0.0"]);
        assert_eq!(project.locked_versions("solo"), ["1.0.0"]);
    }

    /// 2 つの頂点 (fam-a / fam-b) が同じ fam-core を `=` で固定する一族は、どれを
    /// `--precise` で戻しても解決に失敗し、lock は変わらない (1 件ずつでは差し戻せない不具合の再現)
    #[test]
    fn test_lockstep_family_cannot_be_rolled_back_one_by_one() {
        const FAMILY: [&str; 3] = ["fam-core", "fam-a", "fam-b"];
        let registry = LocalRegistry::new();
        for version in ["1.0.0", "1.0.1", "1.0.2"] {
            let pin = format!("={version}");
            registry
                .publish("fam-core", version, &[])
                .publish("fam-a", version, &[("fam-core", pin.as_str())])
                .publish("fam-b", version, &[("fam-core", pin.as_str())]);
        }
        let project = TestProject::new(
            &registry,
            &manifest(&[("fam-a", "=1.0.0"), ("fam-b", "=1.0.0")]),
        );
        project.cargo_ok(&["generate-lockfile"]);
        project.write(
            "Cargo.toml",
            &manifest(&[("fam-a", "1.0.0"), ("fam-b", "1.0.0")]),
        );
        project.cargo_ok(&["update"]);
        for name in FAMILY {
            assert_eq!(project.locked_versions(name), ["1.0.2"], "{name}");
        }

        for name in FAMILY {
            let spec = format!("{name}@1.0.2");
            let (ok, output) = project.cargo(&["update", "-p", &spec, "--precise", "1.0.1"]);
            assert!(!ok, "{spec} を 1.0.1 へ戻せてしまった:\n{output}");
            // spec の書き誤りなどではなく、resolver の衝突で失敗している
            assert!(
                output.contains("failed to select a version"),
                "{spec} の失敗理由が解決の衝突ではない:\n{output}"
            );
            for member in FAMILY {
                assert_eq!(
                    project.locked_versions(member),
                    ["1.0.2"],
                    "{spec} の失敗後に {member} が動いた"
                );
            }
        }
    }

    /// cargo は利用者の CARGO_HOME ではなく、プロジェクト専用の一時ディレクトリを
    /// CARGO_HOME にして、オフラインで動く
    #[test]
    fn test_cargo_runs_with_project_local_cargo_home() {
        let registry = LocalRegistry::new();
        registry.publish("solo", "1.0.0", &[]);
        let project = TestProject::new(&registry, &manifest(&[("solo", "1.0.0")]));
        let other = TestProject::new(&registry, &manifest(&[("solo", "1.0.0")]));
        let envs = project.cargo_envs();

        assert_eq!(
            env_value(&envs, "CARGO_NET_OFFLINE").as_deref(),
            Some(OsStr::new("true"))
        );
        assert_eq!(
            env_value(&envs, "CARGO_TERM_COLOR").as_deref(),
            Some(OsStr::new("never"))
        );
        let cargo_home =
            PathBuf::from(env_value(&envs, "CARGO_HOME").expect("CARGO_HOME を渡していない"));
        // OS の一時ディレクトリの中にあり、利用者の CARGO_HOME (無ければ ~/.cargo) ではない
        assert!(
            cargo_home.starts_with(std::env::temp_dir()),
            "{}",
            cargo_home.display()
        );
        let user_cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")));
        assert_ne!(Some(&cargo_home), user_cargo_home.as_ref());
        // プロジェクトの外に置き、プロジェクトごとに別のディレクトリになる
        assert!(!cargo_home.starts_with(project.path()));
        assert_ne!(
            Some(cargo_home.as_os_str()),
            env_value(&other.cargo_envs(), "CARGO_HOME").as_deref()
        );

        // cargo が実際にこの CARGO_HOME を使う (パッケージキャッシュのロックなどを置く)
        let is_empty = |dir: &Path| fs::read_dir(dir).unwrap().next().is_none();
        assert!(is_empty(&cargo_home), "cargo を起動する前から中身がある");
        project.cargo_ok(&["generate-lockfile"]);
        assert!(!is_empty(&cargo_home), "cargo が CARGO_HOME を使っていない");
        assert_eq!(project.locked_versions("solo"), ["1.0.0"]);
    }

    /// 同名の crate が複数の版で lock されていても semver の昇順で返し
    /// (文字列順なら 2.0.10 が 2.0.9 より前に来る)、crates.io 以外の source は数えない
    #[test]
    fn test_locked_versions_sorts_by_semver_and_skips_other_sources() {
        let registry = LocalRegistry::new();
        let project = TestProject::new(&registry, &manifest(&[]));
        project.write(
            "Cargo.lock",
            r#"version = 4

[[package]]
name = "syn"
version = "2.0.10"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "syn"
version = "1.0.109"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "syn"
version = "2.0.9"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "syn"
version = "3.0.0"
source = "git+https://github.com/dtolnay/syn?branch=master#0123456789abcdef0123456789abcdef01234567"

[[package]]
name = "syn"
version = "4.0.0"
source = "sparse+https://example.com/index/"

[[package]]
name = "syn"
version = "5.0.0"
"#,
        );

        assert_eq!(
            project.locked_versions("syn"),
            ["1.0.109", "2.0.9", "2.0.10"]
        );
        assert!(project.locked_versions("missing").is_empty());
    }
}
