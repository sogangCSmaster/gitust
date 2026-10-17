# Workbench interface

The workbench uses a compact, native desktop-inspired design: quiet surfaces,
system fonts, restrained blue accents, and a clear hierarchy. This is the adopted
Gitust interface direction, developed from the separate visual concept. It is not
an Apple product or an official Apple design.

## Navigation

- **Workspace:** Repositories and Access are global destinations. The sidebar
  brand is a single logo button; repeated product and workspace labels are omitted.
- **Repository:** Open a repository to see Overview, Worktrees, Files, and Changes.
  A visible back action returns to Repositories. The current server exposes one
  configured repository; this screen does not imply multi-repository provisioning.
- **Previews:** Issues, Actions, and Components are grouped separately and remain
  labelled demonstrations. Their sample data never establishes live capabilities.

## Layout and density

- Keep navigation labels, task titles, branches, and filenames on one line when
  they identify an item. Truncate long identifiers with their complete value
  available in a title or the detail view. Do not truncate errors or approval
  evidence merely to reduce height.
- Put actions next to the selected item rather than repeating every action on
  every list row. Keep verification, local approval/merge, and upstream publication
  distinct, with server evidence controlling availability.
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

Responsive viewport checks establish browser layout behavior, not physical device
or assistive-technology certification. The JavaScript bundle remains above the
50 kB gzip target; a compact interface does not establish that performance target.
