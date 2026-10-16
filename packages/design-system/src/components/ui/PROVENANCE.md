# Component provenance

Copied from https://github.com/stefan-karger/solid-ui at `21ba4fa59b775a8bf9ee8249883de34aba1cf807`, `apps/docs/src/registry/ui/`. Upstream is MIT licensed; LICENSE.solid-ui and NOTICE.solid-ui retain its notices. Gitust adaptations include local import paths, theme integration, responsive constraints, and removal of unused sidebar features. Components remain MIT licensed within the AGPL Gitust application.

Dialog and Sheet also use Gitust's `modal-focus.ts` adapter to restore the previous keyboard focus when a controlled modal is opened without a Kobalte trigger. The adapter captures focus when opening, preserves caller autofocus handlers, and leaves non-modal outside-interaction behavior unchanged.
