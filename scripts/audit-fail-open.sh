#!/usr/bin/env bash
# audit-fail-open.sh — mechanical gate against "swallowed error → keep going" (fail-open) patterns
# in security-sensitive crates (auth DB, server, CLI, sandbox, git, sync).
#
# Why: two P1s (revoked sessions resurrected) came from `.ok()` / `let Ok(..) else { return }`
# on lock + reload paths that silently treated I/O failure as "no change".
#
# Rules:
#   * Every hit must either
#       (a) carry an inline justification on the same line, or in the `//` comment block directly
#           above it:   // fail-open-ok: <why ignoring this error cannot weaken auth/isolation/integrity>
#           A marker covers exactly ONE code line plus directly following method-chain
#           continuation lines (lines starting with `.`).
#       (b) or be listed in scripts/fail-open-baseline.txt (legacy, triaged; may only SHRINK —
#           verify.sh fails if it grows, and this script fails on stale entries).
#   * New code must use (a). Do NOT add entries to the baseline to silence new hits.
#
# Baseline key: <file> TAB <previous code line> TAB <hit line> (both trimmed), so a generic
# `.ok()` entry cannot absorb an unrelated new `.ok()` elsewhere in the same file.
#
# Usage:
#   scripts/audit-fail-open.sh          # check (exit 1 on unjustified hits or stale baseline)
#   scripts/audit-fail-open.sh --list   # print all unjustified hits in baseline format
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASELINE="$ROOT/scripts/fail-open-baseline.txt"
CRATES=(gitust-db gitust-server gitust-cli gitust-sandbox gitust-git gitust-sync)
PATTERN='\.ok\(\)|let Ok\(|if let Ok\(|while let Ok\(|unwrap_or_default\(\)|let _ = |\.unwrap_or\(\(\)\)|unwrap_or\((true|false)\)|Err\(_\)[ \t]*=>[ \t]*(return|continue|\{[ \t]*\}|\(\))'

cd "$ROOT"

collect_hits() {
  local dirs=()
  local c
  for c in "${CRATES[@]}"; do
    if [[ ! -d "crates/$c/src" ]]; then
      echo "audit-fail-open: missing crates/$c/src (update CRATES)" >&2
      return 2
    fi
    dirs+=("crates/$c/src")
  done
  local files
  # Exclude test modules: tests.rs, tests_*.rs, */tests/*.
  files="$(find "${dirs[@]}" -type f -name '*.rs' \
    ! -name 'tests.rs' ! -name 'tests_*.rs' ! -path '*/tests/*' | LC_ALL=C sort)"
  if [[ -z "$files" ]]; then
    echo "audit-fail-open: no source files found" >&2
    return 2
  fi
  local f
  while IFS= read -r f; do
    PAT="$PATTERN" awk '
      BEGIN { pat = ENVIRON["PAT"]; marked = 0; prev = "" }
      {
        line = $0
        t = line; sub(/^[ \t]+/, "", t); sub(/[ \t]+$/, "", t)
        if (t ~ /^\/\//) {
          if (t ~ /fail-open-ok:/) marked = 1
          next
        }
        covered = (marked == 1) || (marked == 2 && t ~ /^\./) || (t ~ /fail-open-ok:/)
        if (t ~ pat && !covered) printf "%s\t%s\t%s\n", FILENAME, prev, t
        marked = covered ? 2 : 0
        if (t != "") prev = t
      }' "$f"
  done <<<"$files"
}

if [[ "${1:-}" == "--list" ]]; then
  collect_hits
  exit 0
fi

hits="$(collect_hits)"
baseline=""
if [[ -f "$BASELINE" ]]; then
  baseline="$(grep -v -E '^[[:space:]]*(#|$)' "$BASELINE" || true)"
fi

# Multiset difference: hits - baseline (each baseline line absorbs one identical hit).
stale_file="$(mktemp)"
trap 'rm -f "$stale_file"' EXIT
unjustified="$(
  awk -v stale="$stale_file" '
    NR == FNR { if ($0 != "") allow[$0]++; next }
    $0 == "" { next }
    { if (allow[$0] > 0) { allow[$0]--; next } print }
    END { for (k in allow) if (allow[k] > 0) print k > stale }
  ' <(printf '%s\n' "$baseline") <(printf '%s\n' "$hits")
)"

status=0
if [[ -s "$stale_file" ]]; then
  echo "FAIL-OPEN AUDIT: stale baseline entries (code changed or fixed) — remove them:" >&2
  sed 's/^/  /' "$stale_file" >&2
  status=1
fi

if [[ -n "$unjustified" ]]; then
  echo "FAIL-OPEN AUDIT: unjustified error-swallowing in security crates:" >&2
  while IFS=$'\t' read -r file _prev code; do
    lineno="$(grep -n -F -- "$code" "$file" | head -1 | cut -d: -f1)"
    echo "  $file:${lineno:-?}: $code" >&2
  done <<<"$unjustified"
  cat >&2 <<'EOF'

Propagate the error (`?` / explicit Err) so the operation fails CLOSED, or — only if ignoring
it provably cannot weaken auth, isolation, or data integrity (e.g. best-effort temp cleanup) —
add on the same line or in the comment block directly above:
    // fail-open-ok: <reason>
EOF
  status=1
fi

[[ $status -eq 0 ]] && echo "fail-open audit: OK"
exit $status
