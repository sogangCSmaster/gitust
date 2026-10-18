//! Explicit, bounded loading of one persisted SSH server identity; never creates key material.
use gitust_git::GitEngine;
use russh::keys::PrivateKey;
use std::io;

const ERROR: &str = "SSH host key is unavailable or unsupported";
const MAX_KEY_BYTES: u64 = 64 * 1024;

fn unavailable() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, ERROR)
}

pub(crate) fn load(engine: &GitEngine) -> io::Result<PrivateKey> {
    #[cfg(unix)]
    {
        storage::load(engine)
    }
    #[cfg(not(unix))]
    {
        // fail-open-ok: unsupported platforms always reject key loading below.
        let _ = engine;
        Err(unavailable())
    }
}

#[cfg(unix)]
mod storage {
    use super::*;
    use russh::keys::Algorithm;
    use std::{
        ffi::CStr,
        fs::{self, File, Metadata, OpenOptions},
        io::Read,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::{MetadataExt, OpenOptionsExt},
        },
    };
    use zeroize::Zeroizing;

    fn open_at(parent: &File, leaf: &CStr, flags: libc::c_int) -> io::Result<File> {
        // SAFETY: a fixed NUL-terminated leaf and live parent fd; no path components are followed.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                leaf.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(unavailable());
        }
        // SAFETY: openat returned a fresh owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    fn private(meta: &Metadata, mode: u32) -> bool {
        // SAFETY: geteuid has no arguments or ownership effects.
        meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o7777 == mode
    }
    fn directory(file: &File) -> io::Result<Metadata> {
        let meta = file.metadata().map_err(|_| unavailable())?;
        if !meta.is_dir() || !private(&meta, 0o700) {
            return Err(unavailable());
        }
        Ok(meta)
    }
    fn regular(file: &File) -> io::Result<Metadata> {
        let meta = file.metadata().map_err(|_| unavailable())?;
        if !meta.is_file()
            || !private(&meta, 0o600)
            || meta.nlink() != 1
            || meta.len() == 0
            || meta.len() > MAX_KEY_BYTES
        {
            return Err(unavailable());
        }
        Ok(meta)
    }
    fn same_object(left: &Metadata, right: &Metadata) -> bool {
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    fn openssh_only(text: &str) -> bool {
        let mut lines = text.lines();
        if lines.next() != Some("-----BEGIN OPENSSH PRIVATE KEY-----") {
            return false;
        }
        let (mut body, mut ended) = (false, false);
        for line in lines {
            if line == "-----END OPENSSH PRIVATE KEY-----" {
                if !body || ended {
                    return false;
                }
                ended = true;
            } else if ended
                || line.is_empty()
                || !line
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte))
            {
                return false;
            } else {
                body = true;
            }
        }
        ended
    }
    pub(super) fn load(engine: &GitEngine) -> io::Result<PrivateKey> {
        let _owner = engine.lock_worktree_service().map_err(|_| unavailable())?;
        let identity = engine
            .load_existing_repository_id()
            .map_err(|_| unavailable())?;
        let common_path = engine
            .repository_common_directory()
            .map_err(|_| unavailable())?;
        let common = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&common_path)
            .map_err(|_| unavailable())?;
        let common_meta = common.metadata().map_err(|_| unavailable())?;
        let dir = open_at(&common, c"gitust-ssh", libc::O_RDONLY | libc::O_DIRECTORY)?;
        let dir_meta = directory(&dir)?;
        let file = open_at(&dir, c"host_key", libc::O_RDONLY)?;
        let before = regular(&file)?;
        // Preallocate the whole cap so growing the caller buffer cannot leave prior allocations.
        // The decoder owns additional internal copies; this does not promise to wipe those.
        let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_KEY_BYTES as usize + 1));
        (&file)
            .take(MAX_KEY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable())?;
        let after = regular(&file)?;
        if bytes.len() as u64 != before.len()
            || before.len() != after.len()
            || (
                before.mtime(),
                before.mtime_nsec(),
                before.ctime(),
                before.ctime_nsec(),
            ) != (
                after.mtime(),
                after.mtime_nsec(),
                after.ctime(),
                after.ctime_nsec(),
            )
        {
            return Err(unavailable());
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| unavailable())?;
        if !openssh_only(text) {
            return Err(unavailable());
        }
        let key = russh::keys::decode_secret_key(text, None).map_err(|_| unavailable())?;
        if key.is_encrypted() || key.algorithm() != Algorithm::Ed25519 {
            return Err(unavailable());
        }
        let current_dir = open_at(&common, c"gitust-ssh", libc::O_RDONLY | libc::O_DIRECTORY)?;
        let current_file = open_at(&current_dir, c"host_key", libc::O_RDONLY)?;
        if !same_object(&dir_meta, &directory(&current_dir)?)
            || !same_object(&before, &regular(&current_file)?)
            || !same_object(
                &common_meta,
                &fs::symlink_metadata(&common_path).map_err(|_| unavailable())?,
            )
            || engine
                .load_existing_repository_id()
                .map_err(|_| unavailable())?
                != identity
        {
            return Err(unavailable());
        }
        Ok(key)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use russh::keys::{Algorithm, HashAlg};
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

    struct Fixture {
        _root: tempfile::TempDir,
        engine: GitEngine,
        directory: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let repository = root.path().join("repository.git");
            assert!(crate::tests::fixture_git()
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(["init", "--bare", "--quiet"])
                .arg(&repository)
                .status()
                .unwrap()
                .success());
            let engine = GitEngine::new(&repository);
            engine.load_or_create_repository_id().unwrap();
            Self {
                directory: repository.join("gitust-ssh"),
                engine,
                _root: root,
            }
        }
        fn generate(&self, algorithm: &str, passphrase: &str) {
            fs::create_dir(&self.directory).unwrap();
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700)).unwrap();
            assert!(Command::new("ssh-keygen")
                .args(["-q", "-t", algorithm, "-N", passphrase, "-C", "", "-f"])
                .arg(self.directory.join("host_key"))
                .status()
                .unwrap()
                .success());
            fs::set_permissions(
                self.directory.join("host_key"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        fn refused(&self) {
            match load(&self.engine) {
                Err(error) => assert_eq!(error.to_string(), ERROR),
                Ok(_) => panic!("unsupported host key was accepted"),
            }
        }
    }

    #[test]
    fn persisted_ed25519_identity_is_loaded_without_creation_or_replacement() {
        let fixture = Fixture::new();
        fixture.refused();
        assert!(!fixture.directory.exists());
        fixture.generate("ed25519", "");
        let key = load(&fixture.engine).unwrap();
        assert_eq!(key.algorithm(), Algorithm::Ed25519);
        let native = Command::new("ssh-keygen")
            .args(["-l", "-E", "sha256", "-f"])
            .arg(fixture.directory.join("host_key.pub"))
            .output()
            .unwrap();
        assert!(native.status.success());
        let fingerprint = String::from_utf8(native.stdout)
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_owned();
        assert_eq!(
            key.public_key().fingerprint(HashAlg::Sha256).to_string(),
            fingerprint
        );
        let reloaded = load(&fixture.engine).unwrap();
        assert_eq!(key.public_key(), reloaded.public_key());
    }

    #[test]
    fn configured_key_errors_are_redacted_and_do_not_select_another_key() {
        let encrypted = Fixture::new();
        encrypted.generate("ed25519", "temporary-fixture-passphrase");
        encrypted.refused();
        let unsupported = Fixture::new();
        unsupported.generate("ecdsa", "");
        unsupported.refused();
        let configured = Fixture::new();
        configured.generate("ed25519", "");
        let path = configured.directory.join("host_key");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        configured.refused();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&configured.directory, fs::Permissions::from_mode(0o755)).unwrap();
        configured.refused();
        fs::set_permissions(&configured.directory, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&path, b"not an OpenSSH private key").unwrap();
        configured.refused();
        fs::write(&path, vec![b'A'; MAX_KEY_BYTES as usize + 1]).unwrap();
        configured.refused();
    }
}
