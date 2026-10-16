#!/usr/bin/env bash
# verify.sh — mechanical verification gates (AGENTS.md 7.1 / 7.4 / 7.5).
# Run by .githooks/pre-push; also run manually before asking for review / committing.
# NOTE: This does NOT replace the Gate A/B review subagents (7.5); it only enforces the
# checks a machine can enforce.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

step() { printf '\n==> %s\n' "$*"; }

step "600-line hard gate"
find crates packages -type f \( -name '*.rs' -o -name '*.ts' -o -name '*.tsx' \) \
  -not -path '*/target/*' -not -path '*/node_modules/*' -not -path '*/dist/*' -exec wc -l {} + |
  awk '$2 != "total" && $1 >= 600 { print "OVER_600:", $0; bad = 1 } END { exit bad }'

step "fail-open audit (security crates)"
scripts/audit-fail-open.sh

step "fail-open baseline must remain empty"
count_entries() { { grep -v -E '^[[:space:]]*(#|$)' || true; } | wc -l | tr -d ' '; }
now_count="$(count_entries <scripts/fail-open-baseline.txt)"
if ((now_count != 0)); then
  echo "FAIL: fail-open baseline must remain empty ($now_count entries). Fix the code or justify the precise safe path inline." >&2
  exit 1
fi
scripts/audit-fail-open.sh --self-test

step "cargo fmt --check"
cargo fmt --all -- --check

if ! command -v pnpm >/dev/null 2>&1 || [[ ! -d node_modules ]]; then
  echo "FAIL: pnpm or node_modules missing; run 'pnpm install' (typecheck + bundle gates required)" >&2
  exit 1
fi

step "pnpm typecheck"
pnpm -r typecheck

step "frontend behavioral tests"
pnpm --filter @gitust/design-system test

# gitust-server embeds packages/design-system/dist at compile time, so a stale bundle silently
# ships an old UI in the binary. Preserve original mtimes when the bundle is byte-identical so
# Cargo's incremental cache for gitust-server and gitust-cli stays warm across verify.sh runs.
step "embedded web UI bundle is fresh"
DIST=packages/design-system/dist
dist_hash() { [[ -d "$DIST" ]] && find "$DIST" -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum | shasum; }
TMP_DIST="$(mktemp -d "${TMPDIR:-/tmp}/gitust-dist-check.XXXXXX")"
trap 'rm -rf "$TMP_DIST"' EXIT
[[ -d "$DIST" ]] && cp -pR "$DIST"/. "$TMP_DIST"/
before="$(dist_hash || true)"
pnpm --filter @gitust/design-system build >/dev/null
after="$(dist_hash)"
if [[ "$before" != "$after" ]]; then
  echo "$DIST was stale and has just been rebuilt: review and include the rebuilt bundle." >&2
  exit 1
fi
rm -rf "$DIST"
mkdir -p "$DIST"
cp -pR "$TMP_DIST"/. "$DIST"/

step "cargo clippy -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

step "cargo test"
cargo test --workspace --all-targets

printf '\nverify: all mechanical gates passed\n'
