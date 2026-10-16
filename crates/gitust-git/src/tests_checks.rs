use super::*;

fn write_policy(repo: &Path, contents: &[u8]) {
    fs::write(repo.join(".gitust-checks.json"), contents).unwrap();
    git(repo, &["add", ".gitust-checks.json"]);
    git(repo, &["commit", "-qm", "checks policy"]);
}

#[test]
fn checks_use_committed_branch_and_reject_invalid_or_unbounded_configuration() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());
    let engine = GitEngine::new(tmp.path());
    assert!(engine.required_checks("main").unwrap().is_none());
    assert!(engine.required_checks("missing").is_err());
    fs::write(tmp.path().join(".gitust-checks.json"), "not committed").unwrap();
    assert!(engine.required_checks("main").unwrap().is_none());
    for contents in [
        br#"{"version":2,"commands":["true"]}"#.to_vec(),
        br#"{"version":1,"commands":[]}"#.to_vec(),
        br#"{"version":1,"commands":[" "]}"#.to_vec(),
        br#"{"version":1,"commands":["true"],"unknown":true}"#.to_vec(),
        br#"{"version":1,"commands":["\u0000"]}"#.to_vec(),
        serde_json::json!({"version":1,"commands":vec!["true";9]})
            .to_string()
            .into_bytes(),
        serde_json::json!({"version":1,"commands":["x".repeat(4097)]})
            .to_string()
            .into_bytes(),
        vec![b'x'; 16_385],
        vec![0xff],
    ] {
        write_policy(tmp.path(), &contents);
        assert!(engine.required_checks("main").is_err());
    }
    write_policy(
        tmp.path(),
        br#"{"version":1,"commands":["test -f 'src/lib.rs'"]}"#,
    );
    let checks = engine.required_checks("main").unwrap().unwrap();
    assert_eq!(
        checks.blob_oid,
        git(tmp.path(), &["rev-parse", "main:.gitust-checks.json"])
    );
    assert_eq!(checks.commands, ["test -f 'src/lib.rs'"]);
    git(tmp.path(), &["tag", "main"]);
    assert_eq!(engine.required_checks("main").unwrap(), Some(checks));
}

#[cfg(unix)]
#[test]
fn checks_configuration_must_not_be_a_symlink_or_directory() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());
    let engine = GitEngine::new(tmp.path());
    let config = tmp.path().join(".gitust-checks.json");
    std::os::unix::fs::symlink("src/lib.rs", &config).unwrap();
    git(tmp.path(), &["add", "."]);
    git(tmp.path(), &["commit", "-qm", "symlink policy"]);
    assert!(engine.required_checks("main").is_err());
    fs::remove_file(&config).unwrap();
    fs::create_dir(&config).unwrap();
    fs::write(config.join("child"), "x").unwrap();
    git(tmp.path(), &["add", "."]);
    git(tmp.path(), &["commit", "-qm", "directory policy"]);
    assert!(engine.required_checks("main").is_err());
}
