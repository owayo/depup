use super::*;

/// `home` の下のプロジェクトと、ホームのグローバルな mise の設定 (`[tools]` あり) を作る。
fn project_under_home() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let home = identity(&temp.path().join("home"));
    let project = home.join("src/app");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(home.join(".config/mise")).unwrap();
    fs::write(
        home.join(".config/mise/config.toml"),
        "[tools]\nnode = \"24\"\n",
    )
    .unwrap();
    let project = identity(&project);
    (temp, home, project)
}

/// Issue #97: ホームのグローバルな mise の設定は、親のプロジェクトの設定とみなさない。
/// ホームを `HOME` だけで求めていたため、`HOME` のない Windows ではホームを特定できず、
/// グローバルな設定を親のプロジェクトと誤認して install を準備できなかった。
#[test]
fn the_global_config_in_the_home_is_not_a_parent_project() {
    let (_temp, home, project) = project_under_home();
    assert_eq!(
        check_parent_scope(&project, &project, &[], Some(&home), None),
        Ok(())
    );
    // ホームの場所が分からなくても、mise のグローバルな設定のパスと同じなら対象外
    let global = home.join(".config/mise/config.toml");
    assert_eq!(
        check_parent_scope(&project, &project, &[], None, Some(&global)),
        Ok(())
    );
    // どちらも分からなければ、親のプロジェクトの設定として止める (以前の Windows の挙動)
    assert!(check_parent_scope(&project, &project, &[], None, None).is_err());
}

/// ホームの下でも、グローバルな設定以外の親のプロジェクトの設定は止める。
#[test]
fn a_parent_project_config_under_the_home_still_stops_the_install() {
    let (_temp, home, project) = project_under_home();
    fs::write(home.join("src/mise.toml"), "[tools]\npython = \"3.14\"\n").unwrap();
    let error = check_parent_scope(&project, &project, &[], Some(&home), None).unwrap_err();
    assert!(error.contains("outside the recovery boundary"), "{error}");
}
