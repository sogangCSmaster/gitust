# Solid UI migration

The workbench has been migrated in full to [solid-ui](https://www.solid-ui.com/). SolidJS and the existing Rust HTTP/MCP contracts remain in use. This is a UI replacement, not a Svelte migration.

Completion requires:

- Official solid-ui component source, attribution, Tailwind configuration and theme tokens are installed locally.
- The shell, navigation, command search, dialogs, form controls, badges, cards, tables, switches and avatars use the new component system.
- Every existing screen is migrated: Worktrees and verification results, Issues/PR demo, commit diff, Actions/log demo, Access/accounts/invites/reset/PAT/SSH, Files, and the component reference.
- The former primitive and pattern CSS and static application-specific inline presentation (dynamic viewport geometry remains inline) are removed, rather than retained underneath renamed wrappers.
- Real-server state remains separate from explicit sample data; verification, approval and publication authority remain server-controlled.
- Keyboard shortcuts, command search, focus return, viewer virtualization, theme switching, small-screen layouts and async failure handling are verified.
- Frontend behavioral/component tests, type checking, build, the repository verification script and independent reviews pass. Review limitations must be recorded, not reported as passing gates.
- The browser shows the migrated app connected to the local Rust server. No source-repository commit, push or external publication is part of this UI migration request.

## Validation (2026-10-17)

- The old `src/primitives/`, `tokens/tokens.css`, `patterns/patterns.css` and showcase stylesheet are removed. Source searches find no remaining `ds-*` styles, legacy imports or `--ds-*` tokens. Dynamic virtual-view geometry and sidebar width variables are the intentional inline-style exceptions.
- Official component provenance is recorded in `src/components/ui/PROVENANCE.md`; MIT license and upstream notices are retained beside the copied source. `ui.config.json` follows the upstream CLI schema, and Vite resolves its configured alias.
- Frontend typecheck and 65 behavioral/component tests pass. Actual rendered tests cover sign-in values/password clearing, disabled approval without a verified SHA, plain-text rendering, bounded file views, real modal keyboard isolation, and role-selector typeahead isolation.
- Independent architecture audit: seven rendered probes pass; no remaining P0/P1/P2 in the reviewed frontend scope. Independent frontend security audit: eight DOM probes and six repository component cases pass after fixing modal and selector shortcut leakage. These are frontend-scoped audits, not a new backend/OS/network pentest or completion of the earlier backend Gate B.
- Browser checks cover all seven workspace views at the normal desktop width and 390px mobile width without page-level horizontal overflow. Escape closes the sample dialog and restores focus; command search and Enter navigate to Files; actual repository file selection displays the fetched source.
- The migrated login form successfully signs out and signs back in through Enter submission against the local Rust server. Real verification runs the requested command plus two required Checks; the new approval control fast-forwards the isolated preview repository main to `544b46e0032c860080ce205084748ce9e86b5be9`. The preview checkout is clean and `git fsck --no-dangling` succeeds.
- The rebuilt Rust binary is running locally; SHA-256 comparisons confirm its embedded `/assets/index.js` and `/assets/index.css` exactly match the final Vite build.
- Compact density uses a narrower sidebar and smaller card padding. Adjacent standard/icon/example buttons measure 36px on desktop; the same example action group measures 44px on mobile. Worktree row actions use the shared small size.
- `scripts/verify.sh` exits 0 with the required local macOS sandbox execution permissions. It includes the <600-line gate, fail-open audit, typecheck, tests, embedded-bundle freshness, Rust fmt/clippy and workspace tests.
- The built single-file JavaScript bundle is 314.78 kB / 95.65 kB gzip, and CSS is 46.22 kB / 8.42 kB gzip in the Vite build. This exceeds the project’s 50 kB initial-JavaScript target; this migration does not claim that target is met. The existing embedded-server asset contract still uses one JS file.

Runtime screenshots and local workflow results are retained in the task output. After migration validation, the user explicitly requested commit/push and authorized publication with the earlier Rust Gate B still incomplete; see `../security/check-results-review-2026-10-16.md`. This exception does not waive review for subsequent work.

## Follow-up: deferred modules (2026-10-17)

Access/accounts, the component reference and command search content load only when opened. The command dialog itself opens immediately, preserving the modal keyboard boundary while its content downloads. A failed feature download shows a visible reload action; it does not replace the rest of the workspace or silently report success. Command search receives focus once its module is ready and has an accessible label supplied to the actual command root.

The server build embeds every generated JS/CSS file in a sorted, exact-name asset table. All assets remain inside the single binary; request paths never reach the runtime filesystem. Unknown paths return 404 with security headers rather than the SPA document. Module filenames contain content hashes, and a rebuild tracks the asset directory. The build rejects missing entry assets, unsupported asset types, directories and symlinks.

Vite 6.4.4 on the local macOS development machine reports the entry JavaScript as 233.99 kB / 76.15 kB gzip, compared with 314.78 kB / 95.65 kB gzip before splitting (about 20% less compressed initial JS). CSS remains 46.22 kB / 8.42 kB gzip. Four additional modules are downloaded on demand; the combined download after opening all features is not smaller than the original bundle. The 50 kB initial-JS target is still unmet.

`scripts/verify.sh` exits 0 with 67 frontend cases and the full Rust workspace suite. A module-serving regression first returned 404 with the original fixed routes, then passed for every built JS file with byte-for-byte comparisons. Deferred-feature cases cover immediate modal isolation, search focus, a real command callback and recoverable import rejection.

Independent Gate A and the new static-asset/UI-scoped Gate B both returned CLEAR TO COMMIT with no remaining P0/P1/P2. The latter exercised 18 synthetic GET cases, security headers, assets served after deleting the synthetic build directory, rejected build inputs, and actual pending/rejected command modal behavior. The former also tested closing before module resolution, reopening, focus restoration and shortcut recovery. Older auth/sandbox/network attack surfaces were outside this follow-up's scope and were not retried.

The rebuilt local Rust server serves every generated JS/CSS file byte-for-byte. Browser navigation opens the deferred Components, Access and command palette modules from port 3000 successfully; command search is focused. The existing live development screen on port 4200 remains available. The user subsequently requested a detailed commit and push for this follow-up.
