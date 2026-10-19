# Security upgrade and credential recovery

This upgrade changes secret storage and the treatment of legacy SSO accounts.
It does not close Phase 3, Phase 4 or the alpha acceptance checkpoint. Integration,
platform, backup/restore and independent-review results must be recorded against
the final candidate; this document is not evidence that those checks passed.

## Secret storage contract

SSO assertion signing secrets, TOTP seeds and workflow secrets use AES-256-GCM.
Persistent secrets require `GITUST_SECRET_KEY`: exactly 32 random bytes encoded
as 64 hexadecimal characters, supplied externally to each process opening the
same auth store. The in-memory store generates a random key for that store.

The key is not written to the auth DB or included in installation backups. Keep
it in a separately protected secret manager and supply it through the service
manager's secret/environment mechanism. Do not put it in repository files,
workflow definitions, shell history, logs or test output. Recovery requires both
the supported installation backup and the matching external key. Losing the key
prevents decryption; changing the environment variable alone is not key rotation.

Opening legacy secret-bearing state with the external key migrates the stored
secrets to the new encryption format. A missing key or a wrong key for encrypted
state fails closed. Preserve the original database and stop on an upgrade error;
do not delete encrypted records or generate a replacement key to force startup.

## Before and during upgrade

1. Stop all Gitust processes that write the installation and retain a supported
   offline backup, including the SQLite sidecar/checkpoint. Keep older copies
   access-restricted while deciding their retention and incident-response scope.
2. Generate and store the external key outside the installation. Configure the
   same key for the server and administrator/maintenance processes that open the
   auth store. Ensure the key is recoverable independently of Gitust backups.
3. Start the updated build in a controlled maintenance window. Legacy secret
   migration occurs when the persistent store opens. If opening fails, preserve
   the error and original files without printing secret values; resolve the key
   or storage failure before resuming service.
4. Verify login, expected credential revocation, TOTP enrollment and workflow
   secret access with designated accounts. Rehearse restoration with the matching
   external key on the intended platform. Do not infer success from a unit-test
   run or from the server process starting.

Migration protects newly written storage. It cannot revoke information already
recoverable from an older database or backup. If an old copy was exposed or its
confidentiality cannot be established, rotate SSO assertion signing secrets and
workflow credentials at their issuing systems, and re-enroll TOTP with new seeds.
Re-encrypting the same values does not replace these steps.

## Legacy SSO accounts

Accounts created through the old SSO dummy-password path are migrated to disabled
password login. Their existing sessions, PATs and registered SSH keys are revoked;
users must sign in again through the intended account flow and replace any needed
credentials. An explicit administrator-issued password-reset flow can set a new
password; the old dummy password must never be treated as a usable credential.

This migration is distinct from implementing an identity-provider protocol.
Current SSO accepts a custom HMAC-signed assertion bridge. Provider labels `oidc`
and `ldap` do not perform OIDC discovery/token verification or LDAP connection/bind.
Keep that trust boundary explicit when connecting an external identity system.
