use super::*;

#[test]
fn serve_accepts_explicit_git_write_repositories_without_replacing_read_only_arguments() {
    let result = Cli::try_parse_from([
        "gitust",
        "serve",
        "--repo",
        "/operator/primary.git",
        "--read-only-repo",
        "/operator/reference.git",
        "--git-write-repo",
        "/operator/hosted.git",
    ]);
    assert!(
        result.is_ok(),
        "explicit ordinary Git hosting flag must parse"
    );
}
