use crate::ssh_git_target::{command_service_target, command_target, SshGitService};

#[test]
fn ordinary_git_ssh_urls_select_exact_repository_identity() {
    let id = uuid::Uuid::new_v4();
    for path in [id.to_string(), format!("/{id}.git"), format!("{id}.git")] {
        for command in [
            format!("git-upload-pack '{path}'"),
            format!("git-upload-pack {path}"),
        ] {
            assert_eq!(command_target(command.as_bytes()).unwrap().0, id);
            assert_eq!(
                command_service_target(command.as_bytes()).unwrap(),
                (SshGitService::UploadPack, gitust_core::RepoId(id))
            );
        }
        for command in [
            format!("git-receive-pack '{path}'"),
            format!("git-receive-pack {path}"),
        ] {
            assert_eq!(
                command_service_target(command.as_bytes()).unwrap(),
                (SshGitService::ReceivePack, gitust_core::RepoId(id))
            );
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
    assert!(command_service_target(b"git-receive-pack 'default.git'").is_err());
}
