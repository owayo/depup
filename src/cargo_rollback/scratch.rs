//! 依存解決だけに使う workspace の一時的な写し
//!
//! wasm-bindgen 一族のように互いを `=` で固定し合う crate 群は、`cargo update -p <name>
//! --precise` では 1 件ずつ差し戻せない (`--precise` は依存元を動かさない)。一族をまとめて
//! 解き直すには固定用の依存を足して `cargo update` する必要があるが、利用者の Cargo.toml は
//! 書き換えない。そこで Cargo.lock のある workspace root の写しを一時ディレクトリに作り、
//! 写しの Cargo.toml に固定用の別名依存を足して解き、得られた Cargo.lock だけを元へ戻す。
//!
//! 写しに置くのは解決に要るものだけ:
//! - root と各 member の Cargo.toml。写さない場所を指す path は元の場所の絶対パスへ書き換える
//! - 各 target の空ファイル。実体が無いと cargo が manifest を読めない (中身は解決に使われない)
//! - Cargo.lock
//!
//! cargo は元の root を cwd にして `--manifest-path <写し>/Cargo.toml` で起動する。cargo の
//! 設定ファイルと rust-toolchain は cwd から探されるので、元の `.cargo/config.toml`
//! (source replacement 等) とツールチェーンの指定がそのまま効く。

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fmt::Display;
use std::io;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use toml::{Table, Value};

/// crates.io の依存が `cargo metadata` と Cargo.lock に書かれるときの source。
/// manifest::cargo_lock の定数 (`CRATES_IO_LOCK_SOURCE`) と同じ値
const CRATES_IO_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";

const MANIFEST_FILE: &str = "Cargo.toml";
const LOCK_FILE: &str = "Cargo.lock";

/// 固定用の別名依存のキーの接頭辞 (`depup-age-pin-0`, `depup-age-pin-1`, ...)
const PIN_KEY_PREFIX: &str = "depup-age-pin-";

/// 依存を並べるテーブル。`dev_dependencies` / `build_dependencies` は cargo が今も読む旧綴り
const DEPENDENCY_TABLES: [&str; 5] = [
    "dependencies",
    "dev-dependencies",
    "build-dependencies",
    "dev_dependencies",
    "build_dependencies",
];

/// 配列で並べる target の種類 (`[[bin]]` など)
const TARGET_ARRAYS: [&str; 4] = ["bin", "example", "test", "bench"];

/// 一時ディレクトリの名前が既存と衝突したときに付け直す回数
const TEMP_DIR_ATTEMPTS: u32 = 16;

/// cargo の起動設定。本番は PATH 上の `cargo`、テストは実行中の cargo と一時 CARGO_HOME を使う
#[derive(Debug, Clone)]
pub struct CargoCommand {
    program: OsString,
    envs: Vec<(OsString, OsString)>,
}

impl Default for CargoCommand {
    fn default() -> Self {
        Self {
            program: OsString::from("cargo"),
            envs: Vec::new(),
        }
    }
}

impl CargoCommand {
    pub fn new() -> Self {
        Self::default()
    }

    /// 起動するプログラムを差し替える (ビルダー)
    pub fn program(mut self, program: impl Into<OsString>) -> Self {
        self.program = program.into();
        self
    }

    /// 起動時に足す環境変数 (ビルダー)
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.envs.push((key.into(), value.into()));
        self
    }

    /// `cwd` を作業ディレクトリにした tokio の Command (環境変数を反映し、kill_on_drop(true))
    pub fn command(&self, cwd: &Path) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .current_dir(cwd)
            .envs(self.envs.iter().map(|(key, value)| (key, value)))
            // タイムアウトで待つのをやめたときに、解決中の cargo を残さない
            .kill_on_drop(true);
        command
    }

    /// `args` を `cwd` で実行する。成功なら stdout を返す。失敗なら stderr (前後の空白を
    /// 除いたもの。空なら終了コードの説明) を `CargoError::Failed` で、`timeout` を超えたら
    /// `CargoError::TimedOut` で返す (子プロセスは kill_on_drop で止まる)
    pub async fn run(
        &self,
        cwd: &Path,
        args: &[&OsStr],
        timeout: Duration,
    ) -> Result<String, CargoError> {
        // tokio の output() は stdin を親から引き継ぐので、入力待ちで止まらないよう閉じておく。
        // output() はこの時点で子プロセスを起動し、返る future は Command を借用しない
        let output = self.command(cwd).args(args).stdin(Stdio::null()).output();
        match tokio::time::timeout(timeout, output).await {
            Ok(Ok(output)) if output.status.success() => {
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            }
            Ok(Ok(output)) => Err(CargoError::Failed(failure_message(
                &self.program,
                &String::from_utf8_lossy(&output.stderr),
                output.status,
            ))),
            Ok(Err(e)) => Err(CargoError::Failed(format!(
                "failed to run {}: {e}",
                self.program.to_string_lossy()
            ))),
            Err(_) => Err(CargoError::TimedOut(timeout)),
        }
    }
}

/// cargo の起動の失敗
///
/// 時間切れを cargo 自身の失敗と区別する。まとめ解きで時間切れを「解けなかった」と
/// 扱うと、別の固定の仕方を試し続けて予算を食い、理由も cargo のエラーとして誤って伝わる
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CargoError {
    /// 上限の時間内に終わらなかった
    TimedOut(Duration),
    /// 起動できなかった、または失敗で終わった (値は stderr などの説明)
    Failed(String),
}

impl std::fmt::Display for CargoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CargoError::TimedOut(timeout) => write!(f, "timed out after {}s", timeout.as_secs()),
            CargoError::Failed(message) => f.write_str(message),
        }
    }
}

/// 失敗した cargo の説明。stderr が空なら終了コードで説明する
fn failure_message(program: &OsStr, stderr: &str, status: impl Display) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("{} failed with {status}", program.to_string_lossy())
    } else {
        stderr.to_string()
    }
}

/// 別名依存で固定する 1 件。`requirement` は Cargo の版要求の文字列そのもの
/// (`=0.3.105` のような完全固定も、`>=0.3.0, <=0.3.105` のような範囲も書ける)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgePin {
    pub name: String,
    pub requirement: String,
}

/// Cargo.lock のあるディレクトリ (workspace root) の依存解決に必要なものだけを写した一時ディレクトリ。
/// drop 時に一時ディレクトリを消す
#[derive(Debug)]
pub struct ScratchWorkspace {
    /// 写しの root。drop で中身ごと消える
    dir: TempDir,
    original_root: PathBuf,
    original_lock_path: PathBuf,
    original_lock: String,
    /// 元の Cargo.toml 群 (root と member)
    original_manifests: Vec<OriginalManifest>,
    pin_host: PinHost,
    /// member が crates.io の依存に書いている (実パッケージ名, 版要求)。cargo metadata の出現順
    requirements: Vec<(String, String)>,
}

/// 元の Cargo.toml の位置と、写しを作った時点の内容
#[derive(Debug)]
struct OriginalManifest {
    path: PathBuf,
    content: Vec<u8>,
}

/// 固定用の別名依存を書き込む写しの Cargo.toml
#[derive(Debug)]
struct PinHost {
    path: PathBuf,
    /// 固定の無い状態 (create 直後) の内容
    manifest: Table,
    text: String,
}

impl ScratchWorkspace {
    /// `root` の写しを作る:
    ///  - `cargo metadata --no-deps` (cwd = root) で member を列挙する。`workspace_root` が root と
    ///    一致しなければ Err
    ///  - root の Cargo.toml と各 member の Cargo.toml を、root からの相対配置のまま写しに書く。
    ///    member 以外を指す相対 path は元の場所の絶対パスへ書き換える
    ///  - 各 target の src_path (root の下にあるもの) に空ファイルを置く
    ///  - root の Cargo.toml に `workspace` テーブルが無ければ空で足す
    ///  - root の Cargo.lock を写す (無ければ Err)
    ///
    /// 書いた写しは cargo に読ませ直し、元と同じ member が同じ依存を宣言していることを確かめる。
    /// 再現できない構成 (member の manifest が root の外、`package.workspace` が root 以外を指す、
    /// symlink で配置が変わる member など) は理由付きの Err。
    /// `timeout` は写しを作る処理全体 (cargo の起動 2 回を含む) の上限。呼び出し側の
    /// 時間予算を超えないよう、2 回目の起動には残り時間だけを渡す
    pub async fn create(
        root: &Path,
        cargo: &CargoCommand,
        timeout: Duration,
    ) -> Result<Self, String> {
        let started = std::time::Instant::now();
        let canonical_root = canonical_path(root)
            .map_err(|e| format!("failed to resolve {}: {e}", root.display()))?;
        let original_lock_path = canonical_root.join(LOCK_FILE);
        let original_lock = std::fs::read_to_string(&original_lock_path)
            .map_err(|e| format!("failed to read {}: {e}", original_lock_path.display()))?;

        // 相対パスの root を渡されても cwd と二重に効かないよう、cargo には絶対パスで渡す
        let metadata = read_metadata(
            cargo,
            &canonical_root,
            &canonical_root.join(MANIFEST_FILE),
            timeout,
        )
        .await?;
        let plan = CopyPlan::new(&canonical_root, &metadata)?;

        let dir =
            TempDir::create().map_err(|e| format!("failed to create a scratch directory: {e}"))?;
        let layout = Layout::new(canonical_root, dir.path.clone(), &plan);
        let (original_manifests, pin_host) = layout.write_copy(&plan, &original_lock)?;
        layout
            .verify(cargo, &metadata, timeout.saturating_sub(started.elapsed()))
            .await?;

        Ok(Self {
            dir,
            original_root: root.to_path_buf(),
            original_lock_path,
            original_lock,
            original_manifests,
            pin_host,
            requirements: plan.requirements,
        })
    }

    /// 写しの root ディレクトリ
    pub fn root_dir(&self) -> &Path {
        &self.dir.path
    }

    /// 写しの root の Cargo.toml
    pub fn manifest_path(&self) -> PathBuf {
        self.dir.path.join(MANIFEST_FILE)
    }

    /// 写しの Cargo.lock
    pub fn lock_path(&self) -> PathBuf {
        self.dir.path.join(LOCK_FILE)
    }

    /// 元の root
    pub fn original_root(&self) -> &Path {
        &self.original_root
    }

    /// 元の Cargo.lock として期待する内容 (作成時点の内容。accept_lock で更新される)
    pub fn original_lock(&self) -> &str {
        &self.original_lock
    }

    /// 写しの Cargo.lock を original_lock() の内容に戻す
    pub fn reset_lock(&self) -> io::Result<()> {
        std::fs::write(self.lock_path(), &self.original_lock)
    }

    /// 固定用の別名依存を pin host (root が package ならその Cargo.toml、virtual workspace なら
    /// 相対パス順で最初の member) の `[dependencies]` に
    /// `depup-age-pin-{i} = { package = <name>, version = <requirement>, default-features = false }`
    /// として書く (`requirement` は加工しない)。既存のキーと衝突する番号は飛ばす。
    /// 空スライスなら固定の無い状態 (create 直後の内容) に戻す。
    /// 前回の固定は残さない (毎回 create 直後の内容から書き直す)
    pub fn set_pins(&self, pins: &[AgePin]) -> io::Result<()> {
        if pins.is_empty() {
            return std::fs::write(&self.pin_host.path, &self.pin_host.text);
        }
        let mut manifest = self.pin_host.manifest.clone();
        // dev / build / target 別の依存と同じキーにすると、cargo が別の crate を同じ名前で扱う
        let taken = dependency_keys(&manifest);
        let Value::Table(dependencies) = manifest
            .entry("dependencies")
            .or_insert_with(|| Value::Table(Table::new()))
        else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "`dependencies` in {} is not a table",
                    self.pin_host.path.display()
                ),
            ));
        };
        let mut index = 0usize;
        for pin in pins {
            let key = loop {
                let key = format!("{PIN_KEY_PREFIX}{index}");
                index += 1;
                if !taken.contains(&key) {
                    break key;
                }
            };
            let mut spec = Table::new();
            spec.insert("package".to_string(), Value::String(pin.name.clone()));
            spec.insert(
                "version".to_string(),
                Value::String(pin.requirement.clone()),
            );
            spec.insert("default-features".to_string(), Value::Boolean(false));
            dependencies.insert(key, Value::Table(spec));
        }
        let text = toml::to_string(&manifest).map_err(io::Error::other)?;
        std::fs::write(&self.pin_host.path, text)
    }

    /// 元の Cargo.toml 群 (root と member) と Cargo.lock が、作成時点 (Cargo.lock は original_lock()) から変わっていないか
    pub fn originals_unchanged(&self) -> bool {
        self.original_manifests
            .iter()
            .all(|manifest| std::fs::read(&manifest.path).is_ok_and(|now| now == manifest.content))
            && std::fs::read_to_string(&self.original_lock_path)
                .is_ok_and(|now| now == self.original_lock)
    }

    /// depup 自身が元の Cargo.lock を置き換えた後に呼び、期待する内容を更新する。
    /// 写しの Cargo.lock は変えない (揃えるなら続けて reset_lock を呼ぶ)
    pub fn accept_lock(&mut self, content: String) {
        self.original_lock = content;
    }

    /// member が crates.io の `name` (実パッケージ名) に書いている版要求 (`cargo metadata` の `req`)。重複は除き、出現順
    pub fn direct_requirements(&self, name: &str) -> Vec<String> {
        let mut requirements: Vec<String> = Vec::new();
        for (package, requirement) in &self.requirements {
            if package == name && !requirements.contains(requirement) {
                requirements.push(requirement.clone());
            }
        }
        requirements
    }
}

/// `cargo metadata --no-deps --format-version 1` のうち、写しに使う部分
#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<MetadataPackage>,
    workspace_members: Vec<String>,
    workspace_root: PathBuf,
}

#[derive(Debug, Deserialize)]
struct MetadataPackage {
    id: String,
    name: String,
    version: String,
    manifest_path: PathBuf,
    targets: Vec<MetadataTarget>,
    /// 依存の宣言。写しとの突き合わせでは全項目を比べるので、型を決めずに持つ
    dependencies: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct MetadataTarget {
    src_path: PathBuf,
}

/// `manifest` の workspace を `cargo metadata --no-deps` で読む。解決はしないのでネットワークに出ない
async fn read_metadata(
    cargo: &CargoCommand,
    cwd: &Path,
    manifest: &Path,
    timeout: Duration,
) -> Result<Metadata, String> {
    let args = [
        OsStr::new("metadata"),
        OsStr::new("--no-deps"),
        OsStr::new("--format-version"),
        OsStr::new("1"),
        OsStr::new("--manifest-path"),
        manifest.as_os_str(),
    ];
    let stdout = cargo
        .run(cwd, &args, timeout)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&stdout)
        .map_err(|e| format!("failed to parse the output of `cargo metadata`: {e}"))
}

/// workspace の member (cargo metadata の packages の並び順)
fn workspace_members(metadata: &Metadata) -> impl Iterator<Item = &MetadataPackage> {
    let ids: HashSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    metadata
        .packages
        .iter()
        .filter(move |package| ids.contains(package.id.as_str()))
}

/// crates.io の依存なら (実パッケージ名, 版要求) を返す
fn crates_io_requirement(dependency: &serde_json::Value) -> Option<(String, String)> {
    if dependency.get("source")?.as_str()? != CRATES_IO_SOURCE {
        return None;
    }
    let name = dependency.get("name")?.as_str()?;
    let requirement = dependency.get("req")?.as_str()?;
    Some((name.to_string(), requirement.to_string()))
}

/// 写す manifest 1 件
struct ManifestPlan {
    /// 元の Cargo.toml (cargo metadata が示す位置)
    original: PathBuf,
    /// root から manifest のあるディレクトリまでの相対位置 (root 自身なら空)
    rel_dir: PathBuf,
}

/// 元の workspace から読み取った写しの設計
struct CopyPlan {
    /// 写す manifest。先頭は root の Cargo.toml
    manifests: Vec<ManifestPlan>,
    /// member のディレクトリ: 元 (canonicalize 済み) → root からの相対位置
    member_dirs: HashMap<PathBuf, PathBuf>,
    /// root の下にある target: 元 (canonicalize 済み) → root からの相対位置
    target_files: HashMap<PathBuf, PathBuf>,
    /// 固定用の別名依存を書く manifest の、root からの相対位置
    pin_host: PathBuf,
    requirements: Vec<(String, String)>,
}

impl CopyPlan {
    fn new(root: &Path, metadata: &Metadata) -> Result<Self, String> {
        let workspace_root = canonical_path(&metadata.workspace_root).map_err(|e| {
            format!(
                "failed to resolve {}: {e}",
                metadata.workspace_root.display()
            )
        })?;
        if workspace_root != root {
            return Err(format!(
                "{} is not a workspace root (it belongs to the workspace at {})",
                root.display(),
                metadata.workspace_root.display()
            ));
        }

        let mut manifests = vec![ManifestPlan {
            original: root.join(MANIFEST_FILE),
            rel_dir: PathBuf::new(),
        }];
        let mut member_dirs = HashMap::new();
        let mut target_files = HashMap::new();
        let mut requirements = Vec::new();
        for member in workspace_members(metadata) {
            let Some(manifest_dir) = member.manifest_path.parent() else {
                return Err(format!(
                    "unexpected manifest path of {}: {}",
                    member.name,
                    member.manifest_path.display()
                ));
            };
            let canonical_dir = canonical_path(manifest_dir)
                .map_err(|e| format!("failed to resolve {}: {e}", manifest_dir.display()))?;
            let Ok(rel_dir) = canonical_dir.strip_prefix(root).map(Path::to_path_buf) else {
                return Err(format!(
                    "workspace member {} ({}) is outside the workspace root {}",
                    member.name,
                    member.manifest_path.display(),
                    root.display()
                ));
            };
            // root が package なら root の Cargo.toml は先頭に入れてある
            if !rel_dir.as_os_str().is_empty() {
                manifests.push(ManifestPlan {
                    original: member.manifest_path.clone(),
                    rel_dir: rel_dir.clone(),
                });
            }
            member_dirs.insert(canonical_dir, rel_dir);

            for target in &member.targets {
                let file = canonicalize_lenient(&normalize_path(&target.src_path));
                if let Ok(rel) = file.strip_prefix(root) {
                    let rel = rel.to_path_buf();
                    target_files.insert(file, rel);
                }
            }
            requirements.extend(member.dependencies.iter().filter_map(crates_io_requirement));
        }

        // root が package なら root (相対位置が空で最小)、virtual workspace なら相対パス順で最初の member
        let pin_host = member_dirs
            .values()
            .min()
            .cloned()
            .ok_or_else(|| format!("the workspace at {} has no members", root.display()))?;

        Ok(Self {
            manifests,
            member_dirs,
            target_files,
            pin_host,
            requirements,
        })
    }
}

/// 元の workspace と写しの対応
struct Layout {
    /// 元の root (canonicalize 済み)
    root: PathBuf,
    /// 写しの root (canonicalize 済み)
    copy_root: PathBuf,
    /// 写した member のディレクトリ: 元 (canonicalize 済み) → 写し
    dirs: HashMap<PathBuf, PathBuf>,
    /// 空ファイルを置いた target: 元 (canonicalize 済み) → 写し
    files: HashMap<PathBuf, PathBuf>,
}

/// manifest に書かれた path の指す先の種類
#[derive(Clone, Copy)]
enum PathKind {
    /// path 依存 (package のディレクトリ)
    Package,
    /// target のソースファイル
    Target,
}

impl Layout {
    fn new(root: PathBuf, copy_root: PathBuf, plan: &CopyPlan) -> Self {
        let copy_of = |rel: &PathBuf| join_rel(&copy_root, rel);
        let dirs = plan
            .member_dirs
            .iter()
            .map(|(original, rel)| (original.clone(), copy_of(rel)))
            .collect();
        let files = plan
            .target_files
            .iter()
            .map(|(original, rel)| (original.clone(), copy_of(rel)))
            .collect();
        Self {
            root,
            copy_root,
            dirs,
            files,
        }
    }

    /// 写しを書く。元の Cargo.toml 群 (作成時点の内容) と、固定を書く manifest を返す
    fn write_copy(
        &self,
        plan: &CopyPlan,
        lock: &str,
    ) -> Result<(Vec<OriginalManifest>, PinHost), String> {
        for file in self.files.values() {
            write_file(file, b"")?;
        }

        let mut originals = Vec::new();
        let mut pin_host = None;
        for manifest in &plan.manifests {
            let bytes = std::fs::read(&manifest.original)
                .map_err(|e| format!("failed to read {}: {e}", manifest.original.display()))?;
            let mut table: Table = std::str::from_utf8(&bytes)
                .map_err(|e| e.to_string())
                .and_then(|text| toml::from_str(text).map_err(|e| e.to_string()))
                .map_err(|e| format!("failed to parse {}: {e}", manifest.original.display()))?;

            let copy_dir = join_rel(&self.copy_root, &manifest.rel_dir);
            let dirs = ManifestDirs {
                layout: self,
                original: manifest.original.parent().unwrap_or(&self.root),
                copy: &copy_dir,
            };
            rewrite_manifest(&mut table, &dirs)
                .map_err(|e| format!("{}: {e}", manifest.original.display()))?;
            if manifest.rel_dir.as_os_str().is_empty() && !table.contains_key("workspace") {
                // workspace が無いと、cargo は一時ディレクトリの上位へ workspace root を探しに行く
                table.insert("workspace".to_string(), Value::Table(Table::new()));
            }

            let text = toml::to_string(&table).map_err(|e| {
                format!(
                    "failed to write the copy of {}: {e}",
                    manifest.original.display()
                )
            })?;
            let path = copy_dir.join(MANIFEST_FILE);
            write_file(&path, text.as_bytes())?;
            if manifest.rel_dir == plan.pin_host {
                pin_host = Some(PinHost {
                    path,
                    manifest: table,
                    text,
                });
            }
            originals.push(OriginalManifest {
                path: manifest.original.clone(),
                content: bytes,
            });
        }
        write_file(&self.copy_root.join(LOCK_FILE), lock.as_bytes())?;

        let pin_host = pin_host.ok_or_else(|| {
            format!(
                "no manifest to pin in the workspace at {}",
                self.root.display()
            )
        })?;
        Ok((originals, pin_host))
    }

    /// 写しを cargo に読ませ、元と同じ member が同じ依存を宣言しているか確かめる。
    /// path の書き換えや `[workspace]` の追加で解決の入力が変わっていると、写しで解いた
    /// Cargo.lock を元へ戻したときに元の workspace と合わなくなるので、ここで止める
    async fn verify(
        &self,
        cargo: &CargoCommand,
        original: &Metadata,
        timeout: Duration,
    ) -> Result<(), String> {
        let copy = read_metadata(
            cargo,
            &self.root,
            &self.copy_root.join(MANIFEST_FILE),
            timeout,
        )
        .await
        .map_err(|e| format!("cargo cannot read the scratch copy: {e}"))?;
        if canonicalize_lenient(&copy.workspace_root) != self.copy_root {
            return Err(format!(
                "the scratch copy belongs to the workspace at {}",
                copy.workspace_root.display()
            ));
        }

        // 元の path 依存は、写した member なら写しの位置に読み替えて比べる
        let expected = member_fingerprints(original, &self.root, |path| {
            self.dirs.get(&path).cloned().unwrap_or(path)
        });
        let actual = member_fingerprints(&copy, &self.copy_root, |path| path);
        if expected == actual {
            return Ok(());
        }
        let reason = match expected.iter().zip(&actual).find(|(e, a)| e != a) {
            Some((e, a))
                if (&e.rel_dir, &e.name, &e.version) == (&a.rel_dir, &a.name, &a.version) =>
            {
                format!("the dependencies of {} differ", e.name)
            }
            _ => format!(
                "the members differ (workspace: {}; copy: {})",
                describe_members(&expected),
                describe_members(&actual)
            ),
        };
        Err(format!(
            "the scratch copy does not reproduce the workspace: {reason}"
        ))
    }
}

/// 1 つの manifest の path を書き換えるときの基準
struct ManifestDirs<'a> {
    layout: &'a Layout,
    /// 元の manifest のあるディレクトリ (cargo が相対 path を解決する基準)
    original: &'a Path,
    /// 写しの manifest のあるディレクトリ
    copy: &'a Path,
}

impl ManifestDirs<'_> {
    /// manifest に書かれた path を写しで使う値に直す。写した場所を指すなら、写しの中で同じ場所に
    /// 届く表記 (ふつうは元のままの相対パス)。それ以外は元の場所の絶対パス (canonicalize 済み)
    fn rewrite(&self, value: &str, kind: PathKind) -> Result<String, String> {
        let resolved = canonicalize_lenient(&normalize_path(&self.original.join(value)));
        let copied = match kind {
            PathKind::Package => self.layout.dirs.get(&resolved),
            PathKind::Target => self.layout.files.get(&resolved),
        };
        match copied {
            Some(copied) if normalize_path(&self.copy.join(value)) == *copied => {
                Ok(value.to_string())
            }
            Some(copied) => path_string(copied),
            None => path_string(&resolved),
        }
    }

    /// 依存テーブル (`[dependencies]` や `[patch.<source>]`) の各エントリの `path`
    fn rewrite_dependencies(&self, dependencies: &mut Value) -> Result<(), String> {
        let Value::Table(dependencies) = dependencies else {
            return Ok(());
        };
        for (_, spec) in dependencies.iter_mut() {
            if let Some(Value::String(path)) = spec.get_mut("path") {
                *path = self.rewrite(path, PathKind::Package)?;
            }
        }
        Ok(())
    }

    /// target (`[lib]` や `[[bin]]` の 1 件) の `path`
    fn rewrite_target(&self, target: &mut Value) -> Result<(), String> {
        if let Some(Value::String(path)) = target.get_mut("path") {
            *path = self.rewrite(path, PathKind::Target)?;
        }
        Ok(())
    }

    /// `package.workspace` は root を指すときだけ写せる
    fn rewrite_workspace(&self, value: &str) -> Result<String, String> {
        let resolved = canonicalize_lenient(&normalize_path(&self.original.join(value)));
        if resolved != self.layout.root {
            return Err(format!(
                "package.workspace = {value:?} points to {}, not the workspace root {}",
                resolved.display(),
                self.layout.root.display()
            ));
        }
        if normalize_path(&self.copy.join(value)) == self.layout.copy_root {
            Ok(value.to_string())
        } else {
            path_string(&self.layout.copy_root)
        }
    }
}

/// manifest 内の path (依存・patch・replace・target・build script・package.workspace) を写し用に書き換える
fn rewrite_manifest(manifest: &mut Table, dirs: &ManifestDirs<'_>) -> Result<(), String> {
    for key in DEPENDENCY_TABLES {
        if let Some(dependencies) = manifest.get_mut(key) {
            dirs.rewrite_dependencies(dependencies)?;
        }
    }
    if let Some(Value::Table(platforms)) = manifest.get_mut("target") {
        for (_, platform) in platforms.iter_mut() {
            for key in DEPENDENCY_TABLES {
                if let Some(dependencies) = platform.get_mut(key) {
                    dirs.rewrite_dependencies(dependencies)?;
                }
            }
        }
    }
    if let Some(dependencies) = manifest
        .get_mut("workspace")
        .and_then(|workspace| workspace.get_mut("dependencies"))
    {
        dirs.rewrite_dependencies(dependencies)?;
    }
    if let Some(Value::Table(patches)) = manifest.get_mut("patch") {
        for (_, dependencies) in patches.iter_mut() {
            dirs.rewrite_dependencies(dependencies)?;
        }
    }
    if let Some(replace) = manifest.get_mut("replace") {
        dirs.rewrite_dependencies(replace)?;
    }

    if let Some(lib) = manifest.get_mut("lib") {
        dirs.rewrite_target(lib)?;
    }
    for key in TARGET_ARRAYS {
        if let Some(Value::Array(targets)) = manifest.get_mut(key) {
            for target in targets {
                dirs.rewrite_target(target)?;
            }
        }
    }
    // `project` は `package` の旧名
    for key in ["package", "project"] {
        let Some(Value::Table(package)) = manifest.get_mut(key) else {
            continue;
        };
        if let Some(Value::String(build)) = package.get_mut("build") {
            *build = dirs.rewrite(build, PathKind::Target)?;
        }
        if let Some(Value::String(workspace)) = package.get_mut("workspace") {
            *workspace = dirs.rewrite_workspace(workspace)?;
        }
    }
    Ok(())
}

/// manifest の依存テーブル (`[target.<cfg>]` 配下を含む) に既にあるキー
fn dependency_keys(manifest: &Table) -> HashSet<String> {
    let platforms = manifest
        .get("target")
        .and_then(Value::as_table)
        .into_iter()
        .flat_map(|platforms| platforms.values().filter_map(Value::as_table));
    std::iter::once(manifest)
        .chain(platforms)
        .flat_map(|table| {
            DEPENDENCY_TABLES
                .iter()
                .filter_map(|key| table.get(*key).and_then(Value::as_table))
        })
        .flat_map(|dependencies| dependencies.keys().cloned())
        .collect()
}

/// 解決に効く member の宣言 (配置・名前・版・依存)
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct MemberFingerprint {
    rel_dir: PathBuf,
    name: String,
    version: String,
    /// 依存の宣言を JSON にしたもの (並びを揃えて比べる)
    dependencies: Vec<String>,
}

/// workspace の member の宣言を、root からの相対配置で並べる。
/// path 依存の位置は canonicalize してから `map_path` で比べる基準に揃える
fn member_fingerprints(
    metadata: &Metadata,
    root: &Path,
    map_path: impl Fn(PathBuf) -> PathBuf,
) -> Vec<MemberFingerprint> {
    let mut members: Vec<MemberFingerprint> = workspace_members(metadata)
        .map(|member| {
            let dir = member.manifest_path.parent().unwrap_or(root);
            let dir = canonicalize_lenient(dir);
            let rel_dir = dir.strip_prefix(root).map(Path::to_path_buf).unwrap_or(dir);
            let mut dependencies: Vec<String> = member
                .dependencies
                .iter()
                .map(|dependency| {
                    let mut dependency = dependency.clone();
                    if let Some(serde_json::Value::String(path)) = dependency.get_mut("path") {
                        let mapped = map_path(canonicalize_lenient(&normalize_path(Path::new(
                            path.as_str(),
                        ))));
                        *path = mapped.to_string_lossy().into_owned();
                    }
                    dependency.to_string()
                })
                .collect();
            dependencies.sort();
            MemberFingerprint {
                rel_dir,
                name: member.name.clone(),
                version: member.version.clone(),
                dependencies,
            }
        })
        .collect();
    members.sort();
    members
}

fn describe_members(members: &[MemberFingerprint]) -> String {
    members
        .iter()
        .map(|member| format!("{} at {:?}", member.name, member.rel_dir))
        .collect::<Vec<_>>()
        .join(", ")
}

/// drop で中身ごと消す一時ディレクトリ。
/// tempfile は dev-dependency にしか無く本番コードで使えないので、必要な分だけ自作する
#[derive(Debug)]
struct TempDir {
    /// canonicalize 済みのパス (写しの中のパスと元のパスを同じ基準で比べるため)
    path: PathBuf,
}

impl TempDir {
    /// OS の一時ディレクトリの下に、推測しにくい名前で新しく作る。既存の名前なら作成が失敗するので、
    /// 同じ名前のディレクトリや symlink を先に置かれても乗っ取られない
    fn create() -> io::Result<Self> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        let mut last_error = None;
        for _ in 0..TEMP_DIR_ATTEMPTS {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!(
                "depup-scratch-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            match create_private_dir(&path) {
                Ok(()) => {
                    let mut dir = Self { path };
                    // canonicalize に失敗したら、ここで drop して作ったディレクトリを消す
                    dir.path = canonical_path(&dir.path)?;
                    return Ok(dir);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last_error = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("no name left for a scratch directory")))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // 消せなくても処理は続ける (OS の一時ディレクトリの下なので、いずれ掃除される)
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// 所有者だけが読めるディレクトリを作る (写しには利用者の manifest が入る)。既存なら失敗する
#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

/// ディレクトリを作る。既存なら失敗する (Windows の一時ディレクトリは利用者ごとにあるので、権限は親に従う)
#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

/// 親ディレクトリを作ってからファイルを書く
fn write_file(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// `root` に root からの相対位置 `rel` をつなぐ (空なら root そのもの。末尾に区切りを付けない)
fn join_rel(root: &Path, rel: &Path) -> PathBuf {
    if rel.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    }
}

/// TOML に書くパスの文字列
fn path_string(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

/// `fs::canonicalize` した上で、Windows の `\\?\C:\...` を `C:\...` に戻す
fn canonical_path(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path).map(strip_verbatim_prefix)
}

/// Windows の `\\?\C:\...` を `C:\...` に、`\\?\UNC\server\share\...` を `\\server\share\...` に
/// 戻す (それ以外はそのまま)。`\\?\` の `?` は cargo が workspace の member を展開する glob の
/// ワイルドカードになるため、写しの root に使えない (一時ディレクトリがネットワーク共有に
/// ある環境では UNC 形式になる)
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return path;
    };
    let mut plain = match prefix.kind() {
        Prefix::VerbatimDisk(disk) => PathBuf::from(format!("{}:", char::from(disk))),
        Prefix::VerbatimUNC(server, share) => {
            let mut root = OsString::from(r"\\");
            root.push(server);
            root.push(r"\");
            root.push(share);
            PathBuf::from(root)
        }
        _ => return path,
    };
    plain.push(components.as_path());
    plain
}

/// 存在する最も深い祖先までを canonicalize し、残りの要素をそのままつなぐ。
/// 実体の無いパス (空ファイルを置く前の target など) も、存在するパスと同じ基準で比べられるようにする
fn canonicalize_lenient(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = canonical_path(current) {
            return missing
                .iter()
                .rev()
                .fold(canonical, |joined, name| joined.join(name));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// `.` と `..` を字句的に畳む。cargo が path 依存の位置を決めるのと同じく symlink は辿らない
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// テストで cargo を 1 回起動するときの上限 (CI の遅いランナーでも余裕を持たせる)
    const TIMEOUT: Duration = Duration::from_secs(120);

    /// Cargo プロジェクトを置く一時ディレクトリと、利用者の CARGO_HOME とネットワークに触れない cargo
    struct Sandbox {
        dir: tempfile::TempDir,
        cargo: CargoCommand,
    }

    impl Sandbox {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("cargo-home");
            fs::create_dir_all(&home).unwrap();
            let cargo = CargoCommand::new()
                .program(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                .env("CARGO_HOME", &home)
                .env("CARGO_NET_OFFLINE", "true");
            Self { dir, cargo }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.path(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        async fn run_cargo(&self, cwd: &Path, args: &[&OsStr]) -> Result<String, String> {
            self.cargo
                .run(cwd, args, TIMEOUT)
                .await
                .map_err(|e| e.to_string())
        }

        async fn generate_lockfile(&self, root: &Path) {
            self.run_cargo(
                root,
                &[OsStr::new("generate-lockfile"), OsStr::new("--offline")],
            )
            .await
            .unwrap();
        }

        /// 元の root を cwd にして写しを解決する
        async fn update_copy(&self, scratch: &ScratchWorkspace) -> Result<String, String> {
            let manifest = scratch.manifest_path();
            self.run_cargo(
                scratch.original_root(),
                &[
                    OsStr::new("update"),
                    OsStr::new("--offline"),
                    OsStr::new("--manifest-path"),
                    manifest.as_os_str(),
                ],
            )
            .await
        }

        /// 元の root を cwd にして写しを `cargo metadata --no-deps` で読む
        async fn copy_metadata(&self, scratch: &ScratchWorkspace) -> serde_json::Value {
            let manifest = scratch.manifest_path();
            let stdout = self
                .run_cargo(
                    scratch.original_root(),
                    &[
                        OsStr::new("metadata"),
                        OsStr::new("--no-deps"),
                        OsStr::new("--format-version"),
                        OsStr::new("1"),
                        OsStr::new("--manifest-path"),
                        manifest.as_os_str(),
                    ],
                )
                .await
                .unwrap();
            serde_json::from_str(&stdout).unwrap()
        }
    }

    fn package(name: &str) -> String {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
    }

    fn read_table(path: &Path) -> Table {
        toml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn pin(name: &str, requirement: &str) -> AgePin {
        AgePin {
            name: name.to_string(),
            requirement: requirement.to_string(),
        }
    }

    fn assert_same_file(actual: &str, expected: &Path) {
        let actual = Path::new(actual);
        assert!(actual.is_absolute(), "{}", actual.display());
        assert_eq!(
            fs::canonicalize(actual).unwrap(),
            fs::canonicalize(expected).unwrap()
        );
    }

    #[tokio::test]
    async fn test_create_copies_single_package() {
        let sandbox = Sandbox::new();
        let root = sandbox.path("single");
        let manifest = package("single");
        sandbox.write("single/Cargo.toml", &manifest);
        sandbox.write("single/src/lib.rs", "pub fn answer() -> u32 { 42 }\n");
        sandbox.generate_lockfile(&root).await;
        let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();

        let scratch = ScratchWorkspace::create(&root, &sandbox.cargo, TIMEOUT)
            .await
            .unwrap();
        let copy_root = scratch.root_dir().to_path_buf();
        assert_eq!(scratch.original_root(), root);
        assert_eq!(scratch.manifest_path(), copy_root.join("Cargo.toml"));
        assert_eq!(scratch.lock_path(), copy_root.join("Cargo.lock"));

        // root に [workspace] が足され、target は空ファイルで写る
        let copied = read_table(&scratch.manifest_path());
        assert!(copied["workspace"].is_table());
        assert_eq!(copied["package"]["name"].as_str(), Some("single"));
        assert_eq!(
            fs::read_to_string(copy_root.join("src/lib.rs")).unwrap(),
            ""
        );
        assert_eq!(fs::read_to_string(scratch.lock_path()).unwrap(), lock);
        assert_eq!(scratch.original_lock(), lock);

        // 元の root を cwd にして写しを解決でき、元と同じ Cargo.lock になる
        sandbox.update_copy(&scratch).await.unwrap();
        assert_eq!(fs::read_to_string(scratch.lock_path()).unwrap(), lock);
        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            manifest
        );
        assert_eq!(fs::read_to_string(root.join("Cargo.lock")).unwrap(), lock);
        assert!(scratch.originals_unchanged());

        // drop で写しごと消える
        drop(scratch);
        assert!(!copy_root.exists());
    }

    #[tokio::test]
    async fn test_create_copies_virtual_workspace_and_rewrites_outside_paths() {
        let sandbox = Sandbox::new();
        let root = sandbox.path("ws");
        let root_manifest = "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n";
        // ディレクトリ名の順 (b-first < c-second) とパッケージ名の順 (alpha < zeta) を逆にしておく
        let first_manifest = format!(
            "{}\n[dependencies]\nalpha = {{ path = \"../c-second\" }}\noutside = {{ path = \"../../../outside\" }}\n",
            package("zeta")
        );
        let second_manifest = package("alpha");
        sandbox.write("ws/Cargo.toml", root_manifest);
        sandbox.write("ws/crates/b-first/Cargo.toml", &first_manifest);
        sandbox.write("ws/crates/b-first/src/lib.rs", "");
        sandbox.write("ws/crates/c-second/Cargo.toml", &second_manifest);
        sandbox.write("ws/crates/c-second/src/lib.rs", "");
        sandbox.write("outside/Cargo.toml", &package("outside"));
        sandbox.write("outside/src/lib.rs", "");
        sandbox.generate_lockfile(&root).await;
        let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();

        let scratch = ScratchWorkspace::create(&root, &sandbox.cargo, TIMEOUT)
            .await
            .unwrap();
        let copy_root = scratch.root_dir();

        // member を指す相対 path はそのまま、root の外を指す path は元の場所の絶対パスになる
        let first = read_table(&copy_root.join("crates/b-first/Cargo.toml"));
        assert_eq!(
            first["dependencies"]["alpha"]["path"].as_str(),
            Some("../c-second")
        );
        assert_same_file(
            first["dependencies"]["outside"]["path"].as_str().unwrap(),
            &sandbox.path("outside"),
        );
        assert!(copy_root.join("crates/c-second/Cargo.toml").is_file());
        // virtual workspace の root には何も足さない
        assert_eq!(
            read_table(&scratch.manifest_path()),
            toml::from_str::<Table>(root_manifest).unwrap()
        );

        // 写しでも元と同じ解決になる
        sandbox.update_copy(&scratch).await.unwrap();
        assert_eq!(fs::read_to_string(scratch.lock_path()).unwrap(), lock);

        // 固定はパッケージ名の順ではなく、相対パス順で最初の member に書く
        scratch.set_pins(&[pin("web-sys", "=0.3.105")]).unwrap();
        let first = read_table(&copy_root.join("crates/b-first/Cargo.toml"));
        assert_eq!(
            first["dependencies"]["depup-age-pin-0"]["package"].as_str(),
            Some("web-sys")
        );
        let second = read_table(&copy_root.join("crates/c-second/Cargo.toml"));
        assert!(second.get("dependencies").is_none());

        // 元の Cargo.toml はどれも書き換わらない
        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            root_manifest
        );
        assert_eq!(
            fs::read_to_string(root.join("crates/b-first/Cargo.toml")).unwrap(),
            first_manifest
        );
        assert_eq!(
            fs::read_to_string(root.join("crates/c-second/Cargo.toml")).unwrap(),
            second_manifest
        );
        assert!(scratch.originals_unchanged());
    }

    #[tokio::test]
    async fn test_create_places_empty_targets() {
        let sandbox = Sandbox::new();
        let root = sandbox.path("targets");
        let manifest = r#"[package]
name = "targets"
version = "0.1.0"
edition = "2021"
build = "build.rs"
readme = "README.md"
license-file = "LICENSE.txt"

[lib]
path = "src/mylib.rs"

[[bin]]
name = "tool"
path = "src/tool/main.rs"

[[example]]
name = "shared"
path = "../shared/example.rs"
"#;
        sandbox.write("targets/Cargo.toml", manifest);
        sandbox.write("targets/build.rs", "fn main() {}\n");
        sandbox.write("targets/src/mylib.rs", "pub fn lib() {}\n");
        sandbox.write("targets/src/tool/main.rs", "fn main() {}\n");
        sandbox.write("targets/src/bin/auto.rs", "fn main() {}\n");
        sandbox.write("targets/README.md", "# targets\n");
        sandbox.write("targets/LICENSE.txt", "MIT\n");
        sandbox.write("shared/example.rs", "fn main() {}\n");
        sandbox.generate_lockfile(&root).await;

        let scratch = ScratchWorkspace::create(&root, &sandbox.cargo, TIMEOUT)
            .await
            .unwrap();
        let copy_root = scratch.root_dir();
        // 明示した target も自動検出の target も、同じ位置に空ファイルで置く
        for target in [
            "build.rs",
            "src/mylib.rs",
            "src/tool/main.rs",
            "src/bin/auto.rs",
        ] {
            assert_eq!(
                fs::read_to_string(copy_root.join(target)).unwrap(),
                "",
                "{target}"
            );
        }
        // 解決に使わない readme / license-file は写さない
        assert!(!copy_root.join("README.md").exists());
        assert!(!copy_root.join("LICENSE.txt").exists());

        let copied = read_table(&scratch.manifest_path());
        assert_eq!(copied["package"]["build"].as_str(), Some("build.rs"));
        assert_eq!(copied["lib"]["path"].as_str(), Some("src/mylib.rs"));
        assert_eq!(copied["bin"][0]["path"].as_str(), Some("src/tool/main.rs"));
        // root の外の target は元の場所を指す
        assert_same_file(
            copied["example"][0]["path"].as_str().unwrap(),
            &sandbox.path("shared/example.rs"),
        );

        // readme / license-file の実体が無くても写しを解決できる
        sandbox.update_copy(&scratch).await.unwrap();
        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            manifest
        );
    }

    #[tokio::test]
    async fn test_set_pins_skips_taken_keys_and_restores() {
        let sandbox = Sandbox::new();
        let root = sandbox.path("pinned");
        let manifest = format!(
            "{}\n[dependencies]\ndepup-age-pin-0 = {{ package = \"helper-a\", path = \"vendor/helper-a\" }}\n\n[dev-dependencies]\ndepup-age-pin-2 = {{ package = \"helper-b\", path = \"../helper-b\" }}\n",
            package("pinned")
        );
        sandbox.write("pinned/Cargo.toml", &manifest);
        sandbox.write("pinned/src/lib.rs", "");
        sandbox.write("pinned/vendor/helper-a/Cargo.toml", &package("helper-a"));
        sandbox.write("pinned/vendor/helper-a/src/lib.rs", "");
        sandbox.write("helper-b/Cargo.toml", &package("helper-b"));
        sandbox.write("helper-b/src/lib.rs", "");
        sandbox.generate_lockfile(&root).await;
        let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();

        let scratch = ScratchWorkspace::create(&root, &sandbox.cargo, TIMEOUT)
            .await
            .unwrap();
        let created = fs::read_to_string(scratch.manifest_path()).unwrap();

        // root の下でも member でない path 依存は元の場所を指す (写しの workspace に取り込まない)
        let copied = read_table(&scratch.manifest_path());
        assert_same_file(
            copied["dependencies"]["depup-age-pin-0"]["path"]
                .as_str()
                .unwrap(),
            &root.join("vendor/helper-a"),
        );
        sandbox.update_copy(&scratch).await.unwrap();
        assert_eq!(fs::read_to_string(scratch.lock_path()).unwrap(), lock);

        // 既存の depup-age-pin-0 ([dependencies]) と depup-age-pin-2 ([dev-dependencies]) は飛ばす。
        // 版要求は完全固定も範囲も加工せずに書く
        scratch
            .set_pins(&[
                pin("web-sys", "=0.3.105"),
                pin("js-sys", ">=0.3.0, <=0.3.82"),
            ])
            .unwrap();
        let pinned = read_table(&scratch.manifest_path());
        let dependencies = pinned["dependencies"].as_table().unwrap();
        assert_eq!(
            dependencies["depup-age-pin-0"]["package"].as_str(),
            Some("helper-a")
        );
        let web_sys = dependencies["depup-age-pin-1"].as_table().unwrap();
        assert_eq!(web_sys["package"].as_str(), Some("web-sys"));
        assert_eq!(web_sys["version"].as_str(), Some("=0.3.105"));
        assert_eq!(web_sys["default-features"].as_bool(), Some(false));
        let js_sys = dependencies["depup-age-pin-3"].as_table().unwrap();
        assert_eq!(js_sys["package"].as_str(), Some("js-sys"));
        assert_eq!(js_sys["version"].as_str(), Some(">=0.3.0, <=0.3.82"));
        assert!(!dependencies.contains_key("depup-age-pin-2"));

        // cargo も固定を別名依存として読める
        let metadata = sandbox.copy_metadata(&scratch).await;
        let dependencies = metadata["packages"][0]["dependencies"].as_array().unwrap();
        let renamed = |key: &str| {
            dependencies
                .iter()
                .find(|dependency| dependency["rename"] == key)
                .unwrap()
        };
        let web_sys = renamed("depup-age-pin-1");
        assert_eq!(web_sys["name"], "web-sys");
        assert_eq!(web_sys["req"], "=0.3.105");
        assert_eq!(web_sys["uses_default_features"], false);
        let js_sys = renamed("depup-age-pin-3");
        assert_eq!(js_sys["name"], "js-sys");
        assert_eq!(js_sys["req"], ">=0.3.0, <=0.3.82");

        // 付け直すと前回の固定は残らない
        scratch.set_pins(&[pin("js-sys", "=0.3.82")]).unwrap();
        let repinned = read_table(&scratch.manifest_path());
        let dependencies = repinned["dependencies"].as_table().unwrap();
        assert_eq!(
            dependencies["depup-age-pin-1"]["package"].as_str(),
            Some("js-sys")
        );
        assert!(!dependencies.contains_key("depup-age-pin-3"));

        // 空で呼ぶと create 直後の内容に戻る
        scratch.set_pins(&[]).unwrap();
        assert_eq!(
            fs::read_to_string(scratch.manifest_path()).unwrap(),
            created
        );
        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            manifest
        );
        assert!(scratch.originals_unchanged());
    }

    #[tokio::test]
    async fn test_originals_unchanged_tracks_manifests_and_lock() {
        let sandbox = Sandbox::new();
        let root = sandbox.path("ws");
        let root_manifest = "[workspace]\nmembers = [\"member\"]\nresolver = \"2\"\n";
        let member_manifest = package("member");
        sandbox.write("ws/Cargo.toml", root_manifest);
        sandbox.write("ws/member/Cargo.toml", &member_manifest);
        sandbox.write("ws/member/src/lib.rs", "");
        sandbox.generate_lockfile(&root).await;
        let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();

        let mut scratch = ScratchWorkspace::create(&root, &sandbox.cargo, TIMEOUT)
            .await
            .unwrap();
        assert!(scratch.originals_unchanged());

        // member と root の Cargo.toml を見張る (戻せば true に戻る)
        for (rel, content) in [
            ("ws/member/Cargo.toml", member_manifest.as_str()),
            ("ws/Cargo.toml", root_manifest),
        ] {
            sandbox.write(rel, &format!("{content}# edited\n"));
            assert!(!scratch.originals_unchanged(), "{rel}");
            sandbox.write(rel, content);
            assert!(scratch.originals_unchanged(), "{rel}");
        }

        // Cargo.lock が変わると false。depup 自身が置き換えたなら accept_lock で受け入れる
        let replaced = format!("{lock}# replaced by depup\n");
        fs::write(root.join("Cargo.lock"), &replaced).unwrap();
        assert!(!scratch.originals_unchanged());
        scratch.accept_lock(replaced.clone());
        assert!(scratch.originals_unchanged());
        assert_eq!(scratch.original_lock(), replaced);

        // reset_lock は受け入れた内容へ戻す
        fs::write(scratch.lock_path(), "garbage").unwrap();
        scratch.reset_lock().unwrap();
        assert_eq!(fs::read_to_string(scratch.lock_path()).unwrap(), replaced);
    }

    #[tokio::test]
    async fn test_direct_requirements_lists_crates_io_requirements() {
        let sandbox = Sandbox::new();
        let root = sandbox.path("reg");
        // crates.io を空の local-registry に置き換え、解決なしで registry 依存を宣言できるようにする
        fs::create_dir_all(sandbox.path("empty-registry/index")).unwrap();
        let registry = Value::String(sandbox.path("empty-registry").to_str().unwrap().to_string());
        sandbox.write(
            "reg/.cargo/config.toml",
            &format!(
                "[source.crates-io]\nreplace-with = \"empty\"\n\n[source.empty]\nlocal-registry = {registry}\n\n[registries.other]\nindex = \"https://example.invalid/other-index\"\n"
            ),
        );
        sandbox.write(
            "reg/Cargo.toml",
            "[workspace]\nmembers = [\"app\", \"tool\"]\nresolver = \"2\"\n",
        );
        sandbox.write(
            "reg/app/Cargo.toml",
            &format!(
                "{}\n[dependencies]\nserde = \"1.0.100\"\nserde-old = {{ package = \"serde\", version = \"^1.0.50\" }}\nhelper = {{ path = \"../../helper\" }}\n\n[dev-dependencies]\nserde = \"1.0.100\"\n\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n",
                package("app")
            ),
        );
        sandbox.write("reg/app/src/lib.rs", "");
        sandbox.write(
            "reg/tool/Cargo.toml",
            &format!(
                "{}\n[build-dependencies]\nserde = {{ version = \"1.0.150\", default-features = false }}\nother-serde = {{ package = \"serde\", version = \"9\", registry = \"other\" }}\n",
                package("tool")
            ),
        );
        sandbox.write("reg/tool/src/lib.rs", "");
        sandbox.write("helper/Cargo.toml", &package("helper"));
        sandbox.write("helper/src/lib.rs", "");
        // 解決はしないので、Cargo.lock は写せれば中身を問わない
        sandbox.write("reg/Cargo.lock", "version = 4\n");

        let scratch = ScratchWorkspace::create(&root, &sandbox.cargo, TIMEOUT)
            .await
            .unwrap();
        // 別名 (package = ...) も実パッケージ名で拾い、重複は除き、他の registry の同名 crate は含めない
        let mut serde = scratch.direct_requirements("serde");
        serde.sort();
        assert_eq!(serde, ["^1.0.100", "^1.0.150", "^1.0.50"]);
        assert_eq!(scratch.direct_requirements("libc"), ["^0.2"]);
        assert!(scratch.direct_requirements("helper").is_empty());
        assert!(scratch.direct_requirements("serde-old").is_empty());
        assert!(scratch.direct_requirements("tokio").is_empty());
    }

    #[tokio::test]
    async fn test_create_requires_cargo_lock() {
        let sandbox = Sandbox::new();
        sandbox.write("nolock/Cargo.toml", &package("nolock"));
        sandbox.write("nolock/src/lib.rs", "");
        let error = ScratchWorkspace::create(&sandbox.path("nolock"), &sandbox.cargo, TIMEOUT)
            .await
            .unwrap_err();
        assert!(error.contains("Cargo.lock"), "{error}");
    }

    #[tokio::test]
    async fn test_create_rejects_member_of_outer_workspace() {
        let sandbox = Sandbox::new();
        let root = sandbox.path("ws");
        sandbox.write(
            "ws/Cargo.toml",
            "[workspace]\nmembers = [\"inner\"]\nresolver = \"2\"\n",
        );
        sandbox.write("ws/inner/Cargo.toml", &package("inner"));
        sandbox.write("ws/inner/src/lib.rs", "");
        sandbox.generate_lockfile(&root).await;
        // member のディレクトリに Cargo.lock があっても、workspace root ではないので写さない
        fs::copy(root.join("Cargo.lock"), root.join("inner/Cargo.lock")).unwrap();

        let error = ScratchWorkspace::create(&root.join("inner"), &sandbox.cargo, TIMEOUT)
            .await
            .unwrap_err();
        assert!(error.contains("not a workspace root"), "{error}");
    }

    #[tokio::test]
    async fn test_create_rejects_member_outside_root() {
        let sandbox = Sandbox::new();
        let root = sandbox.path("ws");
        sandbox.write(
            "ws/Cargo.toml",
            "[workspace]\nmembers = [\"../elsewhere\"]\nresolver = \"2\"\n",
        );
        sandbox.write(
            "elsewhere/Cargo.toml",
            &format!("{}workspace = \"../ws\"\n", package("elsewhere")),
        );
        sandbox.write("elsewhere/src/lib.rs", "");
        sandbox.generate_lockfile(&root).await;

        let error = ScratchWorkspace::create(&root, &sandbox.cargo, TIMEOUT)
            .await
            .unwrap_err();
        assert!(error.contains("outside the workspace root"), "{error}");
    }

    #[tokio::test]
    async fn test_create_rewrites_workspace_dependencies_patch_and_target_paths() {
        let sandbox = Sandbox::new();
        let root = sandbox.path("proj");
        let manifest = format!(
            "{}\n[dependencies]\nshared = {{ workspace = true }}\n\n[target.'cfg(unix)'.dependencies]\nunix-only = {{ path = \"../unix-only\" }}\n\n[workspace]\n\n[workspace.dependencies]\nshared = {{ path = \"../shared\" }}\n\n[patch.crates-io]\nserde = {{ path = \"../serde-fork\" }}\n",
            package("proj")
        );
        sandbox.write("proj/Cargo.toml", &manifest);
        sandbox.write("proj/src/lib.rs", "");
        for (dir, name) in [
            ("shared", "shared"),
            ("unix-only", "unix-only"),
            ("serde-fork", "serde"),
        ] {
            sandbox.write(&format!("{dir}/Cargo.toml"), &package(name));
            sandbox.write(&format!("{dir}/src/lib.rs"), "");
        }
        sandbox.generate_lockfile(&root).await;
        let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();

        let scratch = ScratchWorkspace::create(&root, &sandbox.cargo, TIMEOUT)
            .await
            .unwrap();
        let copied = read_table(&scratch.manifest_path());
        assert_same_file(
            copied["workspace"]["dependencies"]["shared"]["path"]
                .as_str()
                .unwrap(),
            &sandbox.path("shared"),
        );
        assert_same_file(
            copied["target"]["cfg(unix)"]["dependencies"]["unix-only"]["path"]
                .as_str()
                .unwrap(),
            &sandbox.path("unix-only"),
        );
        assert_same_file(
            copied["patch"]["crates-io"]["serde"]["path"]
                .as_str()
                .unwrap(),
            &sandbox.path("serde-fork"),
        );

        // 使われない patch も含め、写しで元と同じ Cargo.lock になる
        sandbox.update_copy(&scratch).await.unwrap();
        assert_eq!(fs::read_to_string(scratch.lock_path()).unwrap(), lock);
        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            manifest
        );
    }

    /// member のディレクトリが symlink だと、写しでは glob が member を見つけられない。
    /// そうした食い違いは写しを cargo に読ませ直す段階で Err になる
    #[cfg(unix)]
    #[tokio::test]
    async fn test_create_rejects_copy_that_does_not_reproduce_members() {
        let sandbox = Sandbox::new();
        let root = sandbox.path("ws");
        sandbox.write(
            "ws/Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
        );
        sandbox.write("ws/real/a/Cargo.toml", &package("a"));
        sandbox.write("ws/real/a/src/lib.rs", "");
        fs::create_dir_all(root.join("crates")).unwrap();
        std::os::unix::fs::symlink("../real/a", root.join("crates/a")).unwrap();
        sandbox.generate_lockfile(&root).await;

        let error = ScratchWorkspace::create(&root, &sandbox.cargo, TIMEOUT)
            .await
            .unwrap_err();
        assert!(error.contains("scratch copy"), "{error}");
    }

    #[test]
    fn test_rewrite_manifest_rewrites_every_path_table() {
        let dir = tempfile::tempdir().unwrap();
        let base = canonical_path(dir.path()).unwrap();
        let root = base.join("root");
        let copy_root = base.join("copy");
        let outside = base.join("outside");
        for created in [
            root.join("crates/a/src"),
            root.join("crates/b"),
            outside.clone(),
        ] {
            fs::create_dir_all(created).unwrap();
        }
        let layout = Layout {
            root: root.clone(),
            copy_root: copy_root.clone(),
            dirs: HashMap::from([
                (root.join("crates/a"), copy_root.join("crates/a")),
                (root.join("crates/b"), copy_root.join("crates/b")),
            ]),
            files: HashMap::from([(
                root.join("crates/a/src/lib.rs"),
                copy_root.join("crates/a/src/lib.rs"),
            )]),
        };
        let original = root.join("crates/a");
        let copy = copy_root.join("crates/a");
        let dirs = ManifestDirs {
            layout: &layout,
            original: &original,
            copy: &copy,
        };
        let absolute_b = Value::String(path_string(&root.join("crates/b")).unwrap());
        let mut manifest: Table = toml::from_str(&format!(
            r#"
[package]
name = "a"
build = "../../../outside/build.rs"

[lib]
path = "src/lib.rs"

[[bin]]
name = "tool"
path = "../../../outside/tool.rs"

[[example]]
name = "demo"
path = "../../../outside/demo.rs"

[[test]]
name = "it"
path = "../../../outside/it.rs"

[[bench]]
name = "speed"
path = "../../../outside/speed.rs"

[dependencies]
b = {{ path = "../b" }}
b-absolute = {{ package = "b", path = {absolute_b} }}
serde = "1"

[dev_dependencies]
dev = {{ path = "../../../outside" }}

[build-dependencies]
build = {{ path = "../../../outside" }}

[target.'cfg(unix)'.dependencies]
unix = {{ path = "../../../outside" }}

[patch.crates-io]
serde = {{ path = "../../../outside" }}

[replace]
"foo:0.1.0" = {{ path = "../../../outside" }}
"#
        ))
        .unwrap();
        rewrite_manifest(&mut manifest, &dirs).unwrap();

        // 写した member と、root の下に空ファイルを置いた target は元の相対パスのまま届く
        assert_eq!(manifest["dependencies"]["b"]["path"].as_str(), Some("../b"));
        assert_eq!(manifest["lib"]["path"].as_str(), Some("src/lib.rs"));
        // 写した member を絶対パスで指していたら、写しの member を指し直す
        assert_eq!(
            manifest["dependencies"]["b-absolute"]["path"].as_str(),
            Some(path_string(&copy_root.join("crates/b")).unwrap().as_str())
        );
        // それ以外は元の場所の絶対パス
        let outside_path = |name: &str| path_string(&join_rel(&outside, Path::new(name))).unwrap();
        for (value, expected) in [
            (&manifest["package"]["build"], outside_path("build.rs")),
            (&manifest["bin"][0]["path"], outside_path("tool.rs")),
            (&manifest["example"][0]["path"], outside_path("demo.rs")),
            (&manifest["test"][0]["path"], outside_path("it.rs")),
            (&manifest["bench"][0]["path"], outside_path("speed.rs")),
            (
                &manifest["dev_dependencies"]["dev"]["path"],
                outside_path(""),
            ),
            (
                &manifest["build-dependencies"]["build"]["path"],
                outside_path(""),
            ),
            (
                &manifest["target"]["cfg(unix)"]["dependencies"]["unix"]["path"],
                outside_path(""),
            ),
            (
                &manifest["patch"]["crates-io"]["serde"]["path"],
                outside_path(""),
            ),
            (&manifest["replace"]["foo:0.1.0"]["path"], outside_path("")),
        ] {
            assert_eq!(value.as_str(), Some(expected.as_str()));
        }
        assert_eq!(manifest["dependencies"]["serde"].as_str(), Some("1"));
    }

    #[test]
    fn test_rewrite_manifest_rejects_workspace_pointing_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let base = canonical_path(dir.path()).unwrap();
        let root = base.join("root");
        let copy_root = base.join("copy");
        fs::create_dir_all(root.join("member")).unwrap();
        fs::create_dir_all(base.join("other")).unwrap();
        let layout = Layout {
            root: root.clone(),
            copy_root: copy_root.clone(),
            dirs: HashMap::new(),
            files: HashMap::new(),
        };
        let original = root.join("member");
        let copy = copy_root.join("member");
        let dirs = ManifestDirs {
            layout: &layout,
            original: &original,
            copy: &copy,
        };

        let mut elsewhere: Table =
            toml::from_str("[package]\nname = \"member\"\nworkspace = \"../../other\"\n").unwrap();
        let error = rewrite_manifest(&mut elsewhere, &dirs).unwrap_err();
        assert!(error.contains("package.workspace"), "{error}");

        // root を指すなら写しでも同じ表記で root に届く
        let mut to_root: Table =
            toml::from_str("[package]\nname = \"member\"\nworkspace = \"..\"\n").unwrap();
        rewrite_manifest(&mut to_root, &dirs).unwrap();
        assert_eq!(to_root["package"]["workspace"].as_str(), Some(".."));
    }

    #[tokio::test]
    async fn test_run_returns_stdout_stderr_and_timeout() {
        let sandbox = Sandbox::new();
        let cwd = sandbox.dir.path();

        let version = sandbox
            .run_cargo(cwd, &[OsStr::new("--version")])
            .await
            .unwrap();
        assert!(version.starts_with("cargo "), "{version}");

        let missing = sandbox.path("missing/Cargo.toml");
        let error = sandbox
            .run_cargo(
                cwd,
                &[
                    OsStr::new("locate-project"),
                    OsStr::new("--manifest-path"),
                    missing.as_os_str(),
                ],
            )
            .await
            .unwrap_err();
        assert!(error.contains("does not exist"), "{error}");
        assert_eq!(error, error.trim());

        // 上限 0 なら起動した直後に打ち切る (待つ時間に依存しない)
        let timed_out = sandbox
            .cargo
            .run(cwd, &[OsStr::new("--version")], Duration::ZERO)
            .await
            .unwrap_err();
        assert_eq!(timed_out, CargoError::TimedOut(Duration::ZERO));
        assert_eq!(timed_out.to_string(), "timed out after 0s");
    }

    #[test]
    fn test_failure_message_falls_back_to_exit_status() {
        assert_eq!(
            failure_message(OsStr::new("cargo"), "  error: boom \n", "exit status: 101"),
            "error: boom"
        );
        assert_eq!(
            failure_message(OsStr::new("cargo"), " \n", "exit status: 3"),
            "cargo failed with exit status: 3"
        );
    }

    #[test]
    fn test_normalize_path_folds_dot_segments() {
        assert_eq!(
            normalize_path(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }

    #[test]
    fn test_canonicalize_lenient_keeps_missing_tail() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            canonicalize_lenient(&dir.path().join("missing/file.rs")),
            canonical_path(dir.path()).unwrap().join("missing/file.rs")
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_strip_verbatim_prefix_on_windows() {
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\work\ws")),
            PathBuf::from(r"C:\work\ws")
        );
        // verbatim の UNC は通常の UNC に戻す
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\ws")),
            PathBuf::from(r"\\server\share\ws")
        );
        // verbatim でないものはそのまま
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"C:\work\ws")),
            PathBuf::from(r"C:\work\ws")
        );
    }
}
