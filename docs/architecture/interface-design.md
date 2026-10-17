# Workbench interface

The workbench uses a compact, native desktop-inspired design: quiet surfaces,
system fonts, restrained blue accents, and a clear hierarchy. This is the adopted
Gitust interface direction, developed from the separate visual concept. It is not
an Apple product or an official Apple design.

## Navigation

- **Workspace:** Repositories and Access are global destinations. The sidebar
  brand is a single logo button; repeated product and workspace labels are omitted.
- **Repository:** Open a repository to see Overview, Worktrees, Files, and Changes,
  plus live Actions diagnostics for SiteAdmins. Actions groups retained automatic
  attempts by worktree and opens the exact run in Activity; it has no execution controls.
  A visible back action returns to Repositories. The current server exposes one
  configured repository; this screen does not imply multi-repository provisioning.
- **Previews:** Issues, signed-out Actions, and Components are grouped separately
  as previews. Their sample data never establishes live capabilities.

## Layout and density

- Keep navigation labels, task titles, branches, and filenames on one line when
  they identify an item. Truncate long identifiers with their complete value
  available in a title or the detail view. Do not truncate errors or approval
  evidence merely to reduce height.
- Put actions next to the selected item rather than repeating every action on
  every list row. Keep verification, local approval/merge, and upstream publication
  distinct, with server evidence controlling availability.
- New worktree asks for a task name and base branch, initialized from the selected
  repository. Agent and branch overrides stay under Advanced. Keep inputs and the
  generated branch on failure, prevent duplicate submission, and select the
  confirmed worktree after success. Creating a workspace does not launch an agent.
- Use adjacent controls of equal height. Fine-pointer desktop buttons are compact;
  small screens and coarse pointers retain touch-sized controls. Inputs on coarse
  pointers use a readable size that avoids focus zoom.
- On narrow screens, Worktrees and Files move from a list into a detail screen
  with an explicit back action. Tablet and desktop layouts show both panes when
  space allows. Preserve the selected identity, search, and keyboard focus across
  those transitions.
- Changes uses one commit selector. Full head/base commits and comparison
  provenance remain available under Comparison details; loading and failure
  feedback stay visible outside that disclosure.
- Access separates Account, Credentials, and Members. Invitation and password
  reset links open the relevant account form. Hidden tabs must not retain a
  displayed secret after revocation, logout, or clearing the corresponding state.

## Implementation and evidence

Use the existing SolidJS solid-ui components, SVG sprite, Tailwind semantic
tokens, and `src/styles.css` theme variables in `packages/design-system`.
New screens must follow these shared primitives instead of adding another visual
component system. Both light and dark themes are supported.

Pending or failed view downloads must block state-changing shortcuts, including
commands invoked through the palette. Navigation and recovery remain available.
Nested popup content must retain its owning modal's accessibility and focus scope.

Responsive viewport checks establish browser layout behavior, not physical device
or assistive-technology certification. The JavaScript bundle remains above the
50 kB gzip target; a compact interface does not establish that performance target.

## Adopted interface validation

The [2026-10-17 sanitized workflow evidence](../operations/evidence/workbench-ui-2026-10-17.json)
records a disposable `trunk`-only installation exercised through the real web UI:
creation, committed comparison, command and required Checks, exact-SHA approval,
and a separate explicit publication to a local bare upstream. Independent Git
queries and both repositories' `git fsck --full --strict` confirmed the result.
An invalid base preserved the creation inputs; failed and successful verification
runs appeared separately in history. Mobile popup options remained accessible at
320/375 px, with tablet and desktop layouts checked at 768/1440 px.

The final initial JavaScript static-import closure is 65,358 bytes gzip (203,434
bytes before compression). This improves on the preceding interface but remains
above the 50 KB goal. The evidence identifies an uncommitted follow-up build over
its base revision; it is not fixed-release alpha acceptance, external GitHub
publication, a real IDE-agent session, or measured interaction latency.
