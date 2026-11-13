//! グローバル設定の解釈できない値の警告 (#176)。設定の置き場所を `HOME` で変えられる
//! Unix だけで動かす (Windows のホームはプロファイルのフォルダから決まる)。
#![cfg(unix)]

use std::process::Command;

/// 回帰テスト (#176): 解釈できないグローバル設定の `age` / `max_change` の警告は、マニフェストが
/// 複数あっても 1 回だけ出す
#[test]
fn an_invalid_global_age_is_reported_once() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".config/depup");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "age = \"2weeks\"\nmax_change = \"huge\"\n",
    )
    .unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("package.json"), "{}\n").unwrap();
    std::fs::write(
        project.path().join("pyproject.toml"),
        "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\ndependencies = []\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_depup"))
        .env("HOME", home.path())
        .args(["--dry-run", "--no-osv"])
        .arg(project.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr
            .matches("invalid 'age' value in global config")
            .count(),
        1,
        "{stderr}"
    );
    assert_eq!(
        stderr
            .matches("invalid 'max_change' value in global config")
            .count(),
        1,
        "{stderr}"
    );
}
