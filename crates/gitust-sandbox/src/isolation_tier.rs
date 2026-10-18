//! Stronger execution isolation tier (`IsolationTier::ContainerVm`) policy hardening.

use std::ffi::OsString;

use gitust_core::IsolationTier;

use crate::{PathAccess, PathRule};

pub(crate) fn apply_isolation_tier_policy(
    tier: IsolationTier,
    rules: &mut Vec<PathRule>,
    toolchain_env: &mut Vec<(&'static str, OsString)>,
) {
    toolchain_env.push(("GITUST_ISOLATION_TIER", OsString::from(tier.as_str())));
    if tier == IsolationTier::ContainerVm {
        #[cfg(target_os = "macos")]
        let extra_hidden: &[&str] = &[
            "/Volumes",
            "/Network",
            "/Library/Caches",
            "/Library/Application Support",
            "/private/var/db",
            "/private/var/log",
        ];
        #[cfg(not(target_os = "macos"))]
        let extra_hidden: &[&str] = &[
            "/var/log",
            "/var/cache",
            "/var/lib",
            "/srv",
            "/run",
            "/etc/ssh",
        ];
        for path in extra_hidden {
            rules.push(PathRule::new(*path, PathAccess::Hidden));
        }
        toolchain_env.push(("GITUST_CONTAINER_VM_SANDBOX", OsString::from("1")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_vm_tier_adds_extra_hidden_rules_and_env_markers() {
        let mut host_rules = Vec::new();
        let mut host_env = Vec::new();
        apply_isolation_tier_policy(IsolationTier::HostOs, &mut host_rules, &mut host_env);
        assert!(host_rules.is_empty());
        assert_eq!(
            host_env,
            vec![("GITUST_ISOLATION_TIER", OsString::from("host_os"))]
        );

        let mut vm_rules = Vec::new();
        let mut vm_env = Vec::new();
        apply_isolation_tier_policy(IsolationTier::ContainerVm, &mut vm_rules, &mut vm_env);
        assert!(vm_rules.len() >= 6);
        assert!(vm_rules.iter().all(|r| r.access == PathAccess::Hidden));
        assert!(vm_env.contains(&("GITUST_ISOLATION_TIER", OsString::from("container_vm"))));
        assert!(vm_env.contains(&("GITUST_CONTAINER_VM_SANDBOX", OsString::from("1"))));
    }
}
