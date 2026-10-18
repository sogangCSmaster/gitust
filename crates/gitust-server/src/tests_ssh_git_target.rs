use crate::ssh_git_target::command_target;

#[test]
fn ordinary_git_ssh_urls_select_exact_repository_identity() {
    let id = uuid::Uuid::new_v4();
    for path in [id.to_string(), format!("/{id}.git"), format!("{id}.git")] {
        for command in [
            format!("git-upload-pack '{path}'"),
            format!("git-upload-pack {path}"),
        ] {
            assert_eq!(command_target(command.as_bytes()).unwrap().0, id);
        }
    }
    for command in [
        format!("git-receive-pack '/{id}.git'"),
        "git-upload-pack 'default.git'".into(),
        format!("git-upload-pack '/{id}.git' extra"),
        "git-upload-pack '/00000000-0000-0000-0000-000000000000.git'".into(),
        format!("git-upload-pack '{}'", id.simple()),
        "".into(),
    ] {
        assert!(command_target(command.as_bytes()).is_err());
    }
}
