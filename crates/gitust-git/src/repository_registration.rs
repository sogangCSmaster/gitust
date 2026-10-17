//! The Git-side registration anchor never rewrites the repository's existing identity.
use crate::{GitEngine, GitEngineError};
use gitust_core::{RepoId, RepositoryRegistration};

const RECEIPT: &str = "registration.json";
const PENDING: &str = "registration.pending";
const MAX_BYTES: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationReceipt {
    Absent,
    Pending(RepositoryRegistration),
    Complete(RepositoryRegistration),
}
fn invalid() -> GitEngineError {
    GitEngineError::RepositoryIdentity(
        "repository registration evidence is incomplete or conflicting".into(),
    )
}
#[cfg(unix)]
mod local {
    use super::*;
    use crate::repository_identity::storage;
    use std::{
        fs::File,
        io::{Read, Write},
        os::unix::fs::OpenOptionsExt,
    };
    fn directory(engine: &GitEngine, id: RepoId) -> Result<(File, File), GitEngineError> {
        let common = engine.repository_common_directory()?;
        let parent = File::options()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(common)
            .map_err(|_| invalid())?;
        let directory = storage::open_at(
            &parent,
            &storage::name("gitust-repository")?,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?
        .ok_or_else(invalid)?;
        storage::private_directory(&directory)?;
        if storage::read(&directory)? != id {
            return Err(invalid());
        }
        Ok((parent, directory))
    }
    fn read(
        dir: &File,
        leaf: &str,
        id: RepoId,
    ) -> Result<Option<RepositoryRegistration>, GitEngineError> {
        let Some(mut file) = storage::open_at(dir, &storage::name(leaf)?, libc::O_RDONLY)? else {
            return Ok(None);
        };
        let before = storage::validate_file(&file)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid())?;
        let after = storage::validate_file(&file)?;
        if bytes.len() as u64 != before.len() || before.len() != after.len() {
            return Err(invalid());
        }
        let receipt: RepositoryRegistration =
            serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if !receipt.valid_for(id) {
            return Err(invalid());
        }
        Ok(Some(receipt))
    }
    fn state(dir: &File, id: RepoId) -> Result<RegistrationReceipt, GitEngineError> {
        match (read(dir, RECEIPT, id)?, read(dir, PENDING, id)?) {
            (None, None) => Ok(RegistrationReceipt::Absent),
            (Some(receipt), None) => Ok(RegistrationReceipt::Complete(receipt)),
            (None, Some(pending)) => Ok(RegistrationReceipt::Pending(pending)),
            _ => Err(invalid()),
        }
    }
    impl GitEngine {
        pub fn repository_registration_state(
            &self,
            id: RepoId,
        ) -> Result<RegistrationReceipt, GitEngineError> {
            let ownership = self.lock_worktree_service()?;
            ownership.ensure_available()?;
            let (_, directory) = directory(self, id)?;
            state(&directory, id)
        }

        /// Complete only the exact DB-published association. Unknown staging evidence is never removed.
        pub fn complete_repository_registration(
            &self,
            receipt: RepositoryRegistration,
        ) -> Result<(), GitEngineError> {
            if !receipt.valid_for(receipt.repo_id) {
                return Err(invalid());
            }
            let ownership = self.lock_worktree_service()?;
            ownership.ensure_available()?;
            let (parent, directory) = directory(self, receipt.repo_id)?;
            match state(&directory, receipt.repo_id)? {
                RegistrationReceipt::Complete(existing) if existing == receipt => {
                    storage::sync(&directory)?;
                    return storage::sync(&parent);
                }
                RegistrationReceipt::Absent => {
                    let bytes = serde_json::to_vec(&receipt).map_err(|_| invalid())?;
                    if bytes.len() as u64 > MAX_BYTES {
                        return Err(invalid());
                    }
                    let mut file = storage::open_at(
                        &directory,
                        &storage::name(PENDING)?,
                        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                    )?
                    .ok_or_else(invalid)?;
                    file.write_all(&bytes).map_err(|_| invalid())?;
                    storage::sync(&file)?;
                    storage::validate_file(&file)?;
                    storage::sync(&directory)?;
                }
                RegistrationReceipt::Pending(existing) if existing == receipt => {
                    let file =
                        storage::open_at(&directory, &storage::name(PENDING)?, libc::O_RDONLY)?
                            .ok_or_else(invalid)?;
                    storage::sync(&file)?;
                }
                _ => return Err(invalid()),
            }
            storage::publish(
                &directory,
                &storage::name(PENDING)?,
                &storage::name(RECEIPT)?,
            )?;
            storage::sync(&directory)?;
            storage::sync(&parent)?;
            ownership.ensure_available()
        }
    }
}

#[cfg(not(unix))]
impl GitEngine {
    pub fn repository_registration_state(
        &self,
        _id: RepoId,
    ) -> Result<RegistrationReceipt, GitEngineError> {
        Err(invalid())
    }
    pub fn complete_repository_registration(
        &self,
        _receipt: RepositoryRegistration,
    ) -> Result<(), GitEngineError> {
        Err(invalid())
    }
}
