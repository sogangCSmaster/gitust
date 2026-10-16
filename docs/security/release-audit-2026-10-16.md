# Open-source preparation audit — 2026-10-16

Scope: the working tree based on `40fc66cc0405cb9dd60f0361ea8f19be131038de`, plus all locally reachable Git refs after fetching `origin` and tags. This is a record of checks, not a guarantee that the software is free of vulnerabilities or secrets.

## License and documentation

- The project owner selected `AGPL-3.0-only`. The root license, Cargo workspace metadata, npm package metadata, README, and business direction agree.
- The design-system package includes its own identical license copy; an npm pack dry run confirmed inclusion.
- Earlier versions' existing permissions are not revoked. Third-party dependencies retain their licenses. No commercial exception, contributor copyright assignment, or commercial relicensing grant is assumed.
- The English README distinguishes implemented behavior from the roadmap. AGENTS.md contains contributor workflow and verification rules; product vision, roadmap, and business direction have separate documents.

## Secret inspection

Gitleaks 8.30.1 was downloaded from its official GitHub release and checked against the release SHA-256 checksum. Reports were fully redacted and kept outside the repository. No ignore baseline or inline `gitleaks:allow` suppression was applied.

- Git history: all 20 reachable commits, using `--all --full-history`; zero findings.
- Full object contents: all 404 reachable blobs and all 20 commit objects, including commit messages; zero findings. This supplements diff scanning and includes deleted historical files.
- Release files: tracked and non-ignored untracked files exported to a temporary directory; zero findings.
- Additional inspection found no private-key headers or historical credential-file paths. Credential-like URLs and assignments were traced to test fixtures, runtime-generated test credentials, and documentation examples.
- Local `.env` files are now ignored; example environment files remain eligible for version control and must contain placeholders only.

Detection is limited by the scanner's rules and manual review. Unreachable objects, remote-only refs not fetched, ignored local files, external systems, and GitHub Actions logs/artifacts are outside this audit. Commit author metadata is retained. The repository's visibility was not changed.

## Verification and review

`scripts/verify.sh` exited 0: formatting, Clippy, file-size and fail-closed gates, frontend types/tests, embedded-bundle freshness, and Rust tests passed. Rust: 87 passed, two benchmark tests ignored. Frontend: 33 passed.

Independent code and documentation review found no remaining P0/P1/P2 issues in the reviewed changes after fixes. This is scoped review evidence, not a claim about the entire codebase.

Gate A and Gate B returned **CLEAR TO COMMIT** for the reviewed changes. Gate B ran 15 independent Python HTTP/TCP/process probes against the current local binary, using only temporary repositories, synthetic accounts, and localhost. All passed:

| Surface | Observed result |
|---|---|
| Stream / TOCTOU | A paused request completed after account deactivation was rejected with 403. |
| Process coordination | A second MCP process was rejected; approval and discard during verification returned 409. |
| OAuth / PKCE | Plain PKCE and consumed-code reuse were rejected; an MCP token could ping but could not approve. |
| Authorization | Developer execution and an unknown repository ID were rejected with 403. |
| Sandbox paths | Reading a temporary sentinel through a symlink and writing to it were denied; its contents were preserved. |
| Git input / Checks | Invalid refs were rejected; quoting, comments, and `exit 0` in the requested command did not skip required commands; policy changes invalidated approval. |
| Output bounds | Output beyond 1 MiB was capped without preventing required Checks or normal process completion. |

Initial localhost/Seatbelt permission errors were resolved through the normal execution approval mechanism. No approval-review rejection occurred. An earlier claim of a tool-policy block was corrected. Temporary processes and fixtures were removed. These representative paths do not constitute exhaustive penetration testing or verification on every supported OS; the live execution environment was macOS.
