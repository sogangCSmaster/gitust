import {
  createEffect,
  createSignal,
  For,
  on,
  onCleanup,
  Show,
  type Component,
} from "solid-js";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Icon } from "../components/workspace/icon";
import {
  fetchRepositorySearch,
  type LiveRepositorySearchHit,
  type LiveRepositorySearchReport,
  type SearchMatchKind,
  type SymbolKind,
} from "../showcase/live-code-search-client";

export interface RepositorySearchableFile {
  path: string;
  kind: "file" | "dir";
  content?: string;
  sizeBytes?: number;
  lastCommitMsg?: string;
}

export interface RepositoryCodeSearchPanelProps {
  files: readonly RepositorySearchableFile[];
  liveMode?: boolean;
  repositoryId?: string;
  revision?: string;
  onSelectHit: (path: string, lineNumber: number) => void;
}

const SYMBOL_PATTERNS: ReadonlyArray<{ kind: SymbolKind; regex: RegExp }> = [
  {
    kind: "function",
    regex:
      /\b(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:fn|function)\s+([A-Za-z_][A-Za-z0-9_]*)/,
  },
  {
    kind: "struct",
    regex: /\b(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)/,
  },
  {
    kind: "enum",
    regex: /\b(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)/,
  },
  {
    kind: "trait",
    regex: /\b(?:pub(?:\([^)]*\))?\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)/,
  },
  {
    kind: "interface",
    regex: /\b(?:export\s+)?interface\s+([A-Za-z_][A-Za-z0-9_]*)/,
  },
  {
    kind: "class",
    regex: /\b(?:export\s+)?class\s+([A-Za-z_][A-Za-z0-9_]*)/,
  },
  {
    kind: "type",
    regex:
      /\b(?:pub(?:\([^)]*\))?\s+|export\s+)?type\s+([A-Za-z_][A-Za-z0-9_]*)/,
  },
  {
    kind: "const",
    regex:
      /\b(?:pub(?:\([^)]*\))?\s+|export\s+)?const\s+([A-Za-z_][A-Za-z0-9_]*)/,
  },
];

function extractLineSymbol(
  line: string,
): { symbolName: string; symbolKind: SymbolKind } | null {
  for (const { kind, regex } of SYMBOL_PATTERNS) {
    const match = regex.exec(line);
    if (match && match[1]) {
      return { symbolName: match[1], symbolKind: kind };
    }
  }
  return null;
}

function runLocalSearch(
  files: readonly RepositorySearchableFile[],
  rawQuery: string,
  kind: SearchMatchKind,
  revision: string,
): LiveRepositorySearchReport {
  const q = rawQuery.trim().toLowerCase();
  const symbolHits: LiveRepositorySearchHit[] = [];
  const codeHits: LiveRepositorySearchHit[] = [];
  let indexedFileCount = 0;

  for (const file of files) {
    if (file.kind !== "file") continue;
    indexedFileCount++;
    const text = file.content ?? file.lastCommitMsg ?? file.path;
    const lines = text.split(/\r?\n/);
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i] ?? "";
      const lineNumber = i + 1;
      const sym = extractLineSymbol(line);
      if (
        sym &&
        (kind === "all" || kind === "symbol") &&
        sym.symbolName.toLowerCase().includes(q)
      ) {
        symbolHits.push({
          path: file.path,
          lineNumber,
          snippet: line.trim(),
          matchKind: "symbol",
          symbolName: sym.symbolName,
          symbolKind: sym.symbolKind,
        });
        continue;
      }
      if (
        (kind === "all" || kind === "code") &&
        line.toLowerCase().includes(q)
      ) {
        codeHits.push({
          path: file.path,
          lineNumber,
          snippet: line.trim(),
          matchKind: "code",
          symbolName: sym?.symbolName ?? null,
          symbolKind: sym?.symbolKind ?? null,
        });
      }
    }
  }

  return {
    revision,
    commitSha: "0000000000000000000000000000000000000000",
    query: rawQuery.trim(),
    kind,
    indexedFileCount,
    indexCacheHit: true,
    hits: [...symbolHits, ...codeHits],
  };
}

export const RepositoryCodeSearchPanel: Component<
  RepositoryCodeSearchPanelProps
> = (props) => {
  const [query, setQuery] = createSignal("");
  const [kind, setKind] = createSignal<SearchMatchKind>("all");
  const [report, setReport] = createSignal<LiveRepositorySearchReport | null>(
    null,
  );
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  let requestSeq = 0;
  let activeController: AbortController | null = null;

  const cancelInFlight = () => {
    requestSeq++;
    if (activeController) {
      activeController.abort();
      activeController = null;
    }
  };

  createEffect(
    on(
      () => [props.liveMode, props.repositoryId, props.revision] as const,
      () => {
        cancelInFlight();
        setLoading(false);
        setError(null);
        setReport(null);
      },
      { defer: true },
    ),
  );

  onCleanup(() => {
    cancelInFlight();
  });

  const handleSearch = async (e: Event) => {
    e.preventDefault();
    const q = query().trim();
    if (!q) return;

    cancelInFlight();
    const seq = requestSeq;
    setError(null);
    setReport(null);

    if (!props.liveMode) {
      setReport(
        runLocalSearch(props.files, q, kind(), props.revision ?? "HEAD"),
      );
      return;
    }

    const repoId = props.repositoryId?.trim();
    if (!repoId) {
      setError("Select a connected repository to search code and symbols");
      return;
    }

    if (
      !props.revision ||
      !/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(props.revision)
    ) {
      setError(
        "Repository snapshot is unavailable; refresh files before searching",
      );
      return;
    }
    const ctrl = new AbortController();
    activeController = ctrl;
    setLoading(true);
    try {
      const res = await fetchRepositorySearch(repoId, q, {
        rev: props.revision,
        kind: kind(),
        signal: ctrl.signal,
      });
      if (seq !== requestSeq) return;
      setReport(res);
    } catch (err) {
      if (seq !== requestSeq || ctrl.signal.aborted) return;
      setReport(null);
      setError(
        err instanceof Error ? err.message : "Repository code search failed",
      );
    } finally {
      if (seq === requestSeq) {
        setLoading(false);
      }
    }
  };

  const handleClear = () => {
    cancelInFlight();
    setQuery("");
    setReport(null);
    setError(null);
    setLoading(false);
  };

  return (
    <div
      class="rounded-lg border border-border bg-card p-3 space-y-2.5"
      data-testid="repository-code-search-panel"
    >
      <form onSubmit={handleSearch} class="flex flex-wrap items-center gap-2">
        <div class="relative flex min-w-48 flex-1 items-center">
          <Icon
            name="search"
            size={13}
            class="pointer-events-none absolute left-2.5 text-muted-foreground"
          />
          <input
            type="search"
            aria-label="Search repository code and symbols"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
            placeholder="Search code or symbols (e.g. verify_token, struct)..."
            class="h-8 w-full rounded-md border border-input bg-background pl-8 pr-2.5 text-xs text-foreground"
          />
        </div>

        <div
          role="group"
          aria-label="Search filter"
          class="inline-flex items-center rounded-md border border-border bg-muted/30 p-0.5"
        >
          <Button
            type="button"
            size="sm"
            variant={kind() === "all" ? "secondary" : "ghost"}
            class="h-7 px-2 text-xs"
            onClick={() => setKind("all")}
          >
            All
          </Button>
          <Button
            type="button"
            size="sm"
            variant={kind() === "code" ? "secondary" : "ghost"}
            class="h-7 px-2 text-xs"
            onClick={() => setKind("code")}
          >
            Code
          </Button>
          <Button
            type="button"
            size="sm"
            variant={kind() === "symbol" ? "secondary" : "ghost"}
            class="h-7 px-2 text-xs"
            onClick={() => setKind("symbol")}
          >
            Symbols
          </Button>
        </div>

        <Button type="submit" size="sm" disabled={loading()}>
          <Icon name="search" size={12} />
          Search
        </Button>

        <Show when={query().trim() || report() || error()}>
          <Button type="button" size="sm" variant="ghost" onClick={handleClear}>
            Clear
          </Button>
        </Show>
      </form>

      <Show when={error()}>
        {(msg) => (
          <div
            role="alert"
            class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-1.5 text-xs text-destructive"
          >
            {msg()}
          </div>
        )}
      </Show>

      <Show when={report()}>
        {(res) => (
          <div class="space-y-2 pt-1">
            <div class="flex flex-wrap items-center justify-between gap-2 text-xs">
              <div class="flex flex-wrap items-center gap-1.5">
                <Badge variant="outline" class="font-mono text-[10px]">
                  {res().commitSha.slice(0, 12)}
                </Badge>
                <Badge variant="secondary" class="text-[10px]">
                  {res().indexedFileCount} files indexed
                </Badge>
                <span class="text-muted-foreground">
                  {res().hits.length}{" "}
                  {res().hits.length === 1 ? "match" : "matches"}
                </span>
              </div>
            </div>

            <Show
              when={res().hits.length > 0}
              fallback={
                <p class="text-xs text-muted-foreground">
                  No matches found for &ldquo;{res().query}&rdquo;.
                </p>
              }
            >
              <div class="max-h-56 space-y-1 overflow-y-auto">
                <For each={res().hits}>
                  {(hit) => (
                    <button
                      type="button"
                      onClick={() =>
                        props.onSelectHit(hit.path, hit.lineNumber)
                      }
                      class="flex w-full flex-col gap-1 rounded-md border border-border/70 bg-background px-2.5 py-1.5 text-left text-xs transition-colors hover:bg-muted/40"
                    >
                      <div class="flex flex-wrap items-center justify-between gap-2">
                        <span class="font-mono font-medium text-foreground">
                          {hit.path}:{hit.lineNumber}
                        </span>
                        <Show
                          when={hit.symbolKind}
                          fallback={
                            <Badge
                              variant="outline"
                              class="text-[10px] font-mono"
                            >
                              {hit.matchKind}
                            </Badge>
                          }
                        >
                          {(symKind) => (
                            <Badge
                              variant="secondary"
                              class="text-[10px] font-mono"
                            >
                              {symKind()}
                            </Badge>
                          )}
                        </Show>
                      </div>
                      <code class="block truncate font-mono text-[11px] text-muted-foreground">
                        {hit.snippet}
                      </code>
                    </button>
                  )}
                </For>
              </div>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
};
