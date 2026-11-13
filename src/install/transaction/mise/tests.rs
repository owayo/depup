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

/// 設定ファイル名の接尾辞は、優先度の低い順 (`""` → `.<env>` → `.local` → `.<env>.local`) に並ぶ。
/// 復元の範囲と lock の文脈を作る順はこの順に従う
#[test]
fn config_suffixes_follow_mise_priority_order() {
    let environments = vec!["dev".to_string(), "ci".to_string()];
    assert_eq!(
        config_suffixes(&environments).collect::<Vec<_>>(),
        vec!["", ".dev", ".ci", ".local", ".dev.local", ".ci.local"]
    );
    assert_eq!(config_suffixes(&[]).collect::<Vec<_>>(), vec!["", ".local"]);
}

/// `MISE_ENV` の値は重複も書いたまま残し、明示的に選んだ環境別の設定の環境は、まだ無いものだけを足す
#[test]
fn active_environments_keep_mise_env_and_add_selected_environment_configs() {
    let manifests = [
        PathBuf::from("/p/mise.staging.toml"),
        PathBuf::from("/p/.mise.dev.local.toml"),
        PathBuf::from("/p/mise.local.toml"),
        PathBuf::from("/p/mise.toml"),
    ];
    assert_eq!(
        active_environments(" dev , dev,,ci ", &manifests),
        vec!["dev", "dev", "ci", "staging"]
    );
}

/// 別の場所の設定は、それを含むプロジェクトの設定として読む。`.config/mise/config.toml` は
/// `mise/config.toml` にも要素の末尾で一致するので、長い名前を先に照合する
#[test]
fn alternate_configs_belong_to_the_project_that_contains_them() {
    let project = Path::new("/work/app");
    for (config, parent) in [
        (".config/mise/config.toml", ".config/mise"),
        ("mise/config.toml", "mise"),
        (".mise/config.toml", ".mise"),
        (".config/mise.toml", ".config"),
    ] {
        let config = project.join(config);
        let parent = project.join(parent);
        assert_eq!(
            project_directory(&config, &parent),
            project,
            "{}",
            config.display()
        );
    }
    let config = project.join("mise.toml");
    assert_eq!(project_directory(&config, project), project);
}
