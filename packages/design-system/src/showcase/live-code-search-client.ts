import { getLiveToken } from "./live-repo-context";

export type SearchMatchKind = "all" | "code" | "symbol";

export type SymbolKind =
  | "function"
  | "struct"
  | "enum"
  | "trait"
  | "interface"
  | "class"
  | "type"
  | "const";

export interface LiveRepositorySearchHit {
  path: string;
  lineNumber: number;
  snippet: string;
  matchKind: "code" | "symbol";
  symbolName?: string | null;
  symbolKind?: SymbolKind | null;
}

export interface LiveRepositorySearchReport {
  revision: string;
  commitSha: string;
  query: string;
  kind: SearchMatchKind;
  indexedFileCount: number;
  indexCacheHit: boolean;
  hits: LiveRepositorySearchHit[];
}

const VALID_SYMBOL_KINDS = new Set<SymbolKind>([
  "function",
  "struct",
  "enum",
  "trait",
  "interface",
  "class",
  "type",
  "const",
]);

const isObj = (v: unknown): v is Record<string, unknown> =>
  Boolean(v && typeof v === "object" && !Array.isArray(v));
const isStr = (v: unknown): v is string => typeof v === "string";
const isMatchKind = (v: unknown): v is SearchMatchKind =>
  v === "all" || v === "code" || v === "symbol";
const isSymbolKind = (v: unknown): v is SymbolKind =>
  isStr(v) && VALID_SYMBOL_KINDS.has(v as SymbolKind);
const HEX_SHA_RE = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i;

const parseSearchHit = (raw: unknown): LiveRepositorySearchHit => {
  if (!isObj(raw)) {
    throw new Error("Invalid search hit record");
  }
  const path = raw.path;
  const lineNumber = raw.lineNumber ?? raw.line_number;
  const snippet = raw.snippet;
  const matchKind = raw.matchKind ?? raw.match_kind;
  const symbolName = raw.symbolName ?? raw.symbol_name;
  const symbolKind = raw.symbolKind ?? raw.symbol_kind;

  if (
    !isStr(path) ||
    !path.trim() ||
    typeof lineNumber !== "number" ||
    !Number.isInteger(lineNumber) ||
    lineNumber < 1 ||
    !isStr(snippet) ||
    (matchKind !== "code" && matchKind !== "symbol")
  ) {
    throw new Error("Invalid search hit record");
  }

  return {
    path,
    lineNumber,
    snippet,
    matchKind,
    symbolName: isStr(symbolName) ? symbolName : null,
    symbolKind: isSymbolKind(symbolKind) ? symbolKind : null,
  };
};

const parseSearchReport = (raw: unknown): LiveRepositorySearchReport => {
  if (!isObj(raw)) {
    throw new Error("Invalid repository search response");
  }
  const revision = raw.revision;
  const commitSha = raw.commitSha ?? raw.commit_sha;
  const query = raw.query;
  const kind = raw.kind;
  const indexedFileCount = raw.indexedFileCount ?? raw.indexed_file_count;
  const indexCacheHit = raw.indexCacheHit ?? raw.index_cache_hit;
  const hits = raw.hits;

  if (
    !isStr(revision) ||
    !isStr(commitSha) ||
    !HEX_SHA_RE.test(commitSha) ||
    !isStr(query) ||
    !isMatchKind(kind) ||
    typeof indexedFileCount !== "number" ||
    !Number.isInteger(indexedFileCount) ||
    indexedFileCount < 0 ||
    typeof indexCacheHit !== "boolean" ||
    !Array.isArray(hits)
  ) {
    throw new Error("Invalid repository search response");
  }

  return {
    revision,
    commitSha,
    query,
    kind,
    indexedFileCount,
    indexCacheHit,
    hits: hits.map(parseSearchHit),
  };
};

export async function fetchRepositorySearch(
  repositoryId: string,
  query: string,
  options?: {
    rev?: string;
    kind?: SearchMatchKind;
    signal?: AbortSignal;
    fetcher?: typeof fetch;
  },
): Promise<LiveRepositorySearchReport> {
  const repoId = repositoryId.trim();
  const q = query.trim();
  if (!repoId) {
    throw new Error("Repository ID is required for code search");
  }
  if (!q) {
    throw new Error("Search query is required");
  }

  const kind: SearchMatchKind = options?.kind ?? "all";
  const params = new URLSearchParams();
  params.set("q", q);
  params.set("kind", kind);
  if (options?.rev?.trim()) {
    params.set("rev", options.rev.trim());
  }

  const headers: Record<string, string> = {};
  const token = getLiveToken();
  if (token) {
    headers.Authorization = `Bearer ${token}`;
  }

  const fn = options?.fetcher ?? fetch;
  const res = await fn(
    `/api/v1/repos/${encodeURIComponent(repoId)}/search?${params.toString()}`,
    {
      method: "GET",
      headers,
      cache: "no-store",
      signal: options?.signal,
    },
  );

  if (!res.ok) {
    const detail = (await res.text().catch(() => "")).trim();
    throw new Error(
      `Unable to search repository (HTTP ${res.status}${detail ? `: ${detail}` : ""})`,
    );
  }

  const report = parseSearchReport(await res.json());
  const requestedRevision = options?.rev?.trim();
  if (
    requestedRevision &&
    HEX_SHA_RE.test(requestedRevision) &&
    report.commitSha.toLowerCase() !== requestedRevision.toLowerCase()
  ) {
    throw new Error(
      "Search returned a different repository snapshot; refresh files",
    );
  }
  return report;
}
