//! Reject isolation guarantees that have no execution backend.

use gitust_core::IsolationTier;

use crate::SandboxError;

pub(crate) fn validate_isolation_tier(tier: IsolationTier) -> Result<(), SandboxError> {
    match tier {
        IsolationTier::HostOs => Ok(()),
        IsolationTier::ContainerVm => Err(SandboxError::IsolationUnavailable(
            "container_vm is unsupported: no VM execution backend is configured".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_implemented_isolation_tiers_are_accepted() {
        assert!(validate_isolation_tier(IsolationTier::HostOs).is_ok());
        assert!(matches!(
            validate_isolation_tier(IsolationTier::ContainerVm),
            Err(SandboxError::IsolationUnavailable(_))
        ));
    }
}
