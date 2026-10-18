# Web bundle measurements

The initial JavaScript target is **less than 50,000 gzip bytes**. It remains unmet.
Measure every HTML module script and modulepreload dependency together with its
transitive static imports. Moving a dependency into another eagerly loaded file
does not reduce this total.

With locked dependencies installed and Node.js 24, run from the repository root:

```sh
pnpm --filter @gitust/design-system exec node scripts/measure-bundle.mjs
```

The command uses the production Vite configuration with private temporary output
and cache directories. It reads final emitted bytes after the build, compresses
each JavaScript file independently, sums each closure and removes its own scratch
directory. It does not update the committed embedded bundle. The report includes
the complete initial static closure and the cumulative first Repository Overview
closure. Gzip levels are measurements, not HTTP transfer guarantees: actual server
compression, headers, caching and connection overhead are outside this calculation.

## 2026-10-18 import and Overview boundary

The [recorded candidate](../operations/evidence/initial-js-local-2026-10-18.json)
is based on `ada787a8ab49b0b6493815b3088c7fa3a7f4498c`. Both builds use Node.js
24.18.1, Vite 6.4.4 and zlib 1.3.1-e00f703 on Darwin 25.5.0 arm64, Apple M4 Pro.

| Complete JavaScript closure | Before gzip9 | After gzip9 | Difference |
|---|---:|---:|---:|
| Initial repository catalog | 73,656 B | 70,031 B | -3,625 B (-4.92%) |
| First Repository Overview, cumulative | 73,656 B | 73,888 B | +232 B |

The initial raw bytes fall from 231,160 to 217,774. At gzip6, the initial closure
falls from 73,733 to 70,083 bytes. Both initial builds have one JavaScript file and
no HTML modulepreload links. The unchanged stylesheet is 48,967 raw bytes and is
excluded from the JavaScript budget.

The application imports its runtime primitives directly instead of through the
public component barrel, while keeping the package's public exports unchanged.
Repository sample metadata no longer imports a rendering module. The catalog
remains immediate; Overview tools download when the user opens a repository.
Authentication, repository and task clients, request epochs, shortcut registration
and creation controllers retain their existing root lifetime.

Static imports alone do not prove startup behavior. An actual logged-in fresh page
requested only `index.js` and `index.css` from the asset server; the Overview chunk
was first requested after opening a repository. User-initiated Access, clone and
default-branch dialogs have separate optional dependencies. Invite/reset URLs can
open the existing Access view during startup and are a different cohort.

Actual module-delay and 404 fixtures also verified catalog navigation, selection
of the current repository after a pending download, visible failure and recovery
through **Reload application**. The failed import is not described as an immediate
in-place retry. These checks establish the boundary and recovery behavior; no
startup latency, input latency, physical-device or network-throughput target is
claimed. See the evidence record for native browser scope and independent reviews.

## Repository access-scope candidate, 2026-10-18

The subsequent [access-scope feature](../operations/evidence/repository-access-local-2026-10-18.json)
is based on `1fee2f7d77dba8e57de30022cdf256b3edc6aef1` and uses the same measurement
environment and complete-closure method. It adds 253 initial gzip9 bytes: the
initial catalog is 70,284 bytes, first Overview is 74,405 cumulative bytes and first
access dialog is 98,204 cumulative bytes. At gzip6 those totals are 70,333, 74,455
and 98,265 bytes. The initial target remains unmet; these are transfer-size
measurements rather than latency claims. The form's select implementation remains
an optional dependency downloaded on demand.

## Secondary clone/fetch candidate, 2026-10-18

The [read-only transport candidate](../operations/evidence/read-only-git-local-2026-10-18.json)
is based on `691fdd24e73f7ddfcce91904a5b1492df5cc2a0b`. The same maintained measurement
command and environment give 70,350 initial gzip9 bytes (+66) and 74,532 cumulative
bytes for the first Overview, which already contains the compact Clone dialog.
At gzip6 these totals are 70,395 and 74,579 bytes. No additional Clone chunk is
required. The initial 50,000-byte target remains unmet; these measurements do not
establish startup or interaction latency.


## Owning-group invitation candidate, 2026-10-18

The [owning-group invitation candidate](../operations/evidence/repository-group-invites-local-2026-10-18.json) is based on
`ee313662d8e38a0a7c112a47e1df342b5ad50d3e`. The same maintained command measures
70,547 initial gzip9 bytes (+197) and 75,053 cumulative bytes for the first
Overview. At gzip6, those totals are 70,597 and 75,107 bytes. Invitation form
rendering and response parsing remain lazy; the existing workspace controller
retains only the non-secret retry draft across same-repository navigation.
The initial 50,000-byte target remains unmet. These emitted-byte measurements do
not establish startup or interaction latency.

## Additional Git hosting candidate, 2026-10-18

Based on `9844321842eb28ee38b3c724f64eb9ed79245af7`, the same maintained measurement
command and environment give 70,482 initial gzip9 bytes (+47) and 75,262 cumulative
bytes for the first Overview (+114). At gzip6 the totals are 70,528 and 75,317 bytes.
The optional push hint and scope copy add no new chunk. The initial 50,000-byte target
remains unmet; these emitted-byte measurements do not establish interaction latency.

## Secondary default-branch management, 2026-10-18

Based on `74b271b76b86325bb509274c7b83ea9e536f16ae`, removing the redundant primary-only
UI conditions yields 70,467 initial gzip9 bytes (-15) and 75,235 cumulative bytes
for the first Overview (-27), using the same maintained command and environment.
The existing default-branch form stays lazy. The 50,000-byte initial target remains
unmet; these are emitted-byte measurements, not interaction or network timing.

## Local Checks history inspection, 2026-10-18

Based on `0218f86aff20c37127ace173fd792550e55a14b6`, the same maintained command and
environment measure 70,555 initial gzip9 bytes (+88) and 75,322 cumulative bytes
for first Overview (+87). At gzip6 these totals are 70,602 and 75,376 bytes.
The initial closure remains one JavaScript file with no HTML modulepreload roots;
Local Checks is outside both measured closures and loads through Actions on demand.
The 50,000-byte initial target remains unmet. These emitted-byte measurements do
not establish startup, interaction or network timing. See the
[candidate evidence](../operations/evidence/local-checks-web-2026-10-18.json).

## Explicit web Local Checks, 2026-10-18

Based on `e181f86e723badfde4bfd4a8818a12d67db555af`, the same maintained command
measures 70,735 initial gzip9 bytes (+180) and 75,502 cumulative bytes for first
Overview (+180). Opening Actions uses 84,842 cumulative bytes; then opening Local
Checks uses 92,853 bytes. The execution client and local result view remain outside
the initial and Overview closures. The initial 50,000-byte target remains unmet.
These emitted-byte measurements do not establish startup or interaction timing.

## Default-branch automatic status, 2026-10-18

Based on `f1c2731ca40e3c75cc5d052b2f1613f040ab706b`, the same maintained command
measures 70,791 initial gzip9 bytes (+56) and 75,558 cumulative bytes for first
Overview (+56). The normal Overview → Actions → Local Checks path totals
99,654 bytes, compared with 97,620 on that same path before this change (+2,034).
The direct initial → Actions → Local Checks closure is 94,887 bytes; it excludes
the previously visited Overview and is a different cohort. The new lifecycle
client and panel remain outside the initial and Overview closures. The initial
50,000-byte target remains unmet; these are emitted-byte totals, not latency claims.
