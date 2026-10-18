import { getLiveToken } from "./live-repo-context";

export interface LiveWorkflowJobDefinition {
  id: string;
  name: string;
  needs: string[];
  runs_on?: string;
  isolation_tier?: "host_os" | "container_vm";
  matrix?: Record<string, string[]> | null;
  secrets?: string[];
  cache_key?: string | null;
  artifact_name?: string | null;
  steps_count: number;
}
export interface LiveWorkflowDefinition {
  name: string;
  on: string[];
  jobs: LiveWorkflowJobDefinition[];
}
export interface LiveWorkflowArtifact {
  name: string;
  path: string;
  size_bytes: number;
  content_preview?: string | null;
}
export interface LiveWorkflowJobRun {
  job_id: string;
  display_name: string;
  needs: string[];
  matrix_values: Record<string, string>;
  runner_id?: string | null;
  isolation_tier?: "host_os" | "container_vm";
  status: "passed" | "failed" | "skipped";
  cache_hit: boolean;
  artifacts: LiveWorkflowArtifact[];
  redacted_output: string;
  duration_ms: number;
}
export interface LiveWorkflowRunRecord {
  run_id: string;
  repo_id: string;
  workflow_name: string;
  commit_sha: string;
  source_ref: string;
  status: "passed" | "failed";
  jobs: LiveWorkflowJobRun[];
  created_at: string;
}
export interface LiveRepositoryWorkflowsOverview {
  workflows: LiveWorkflowDefinition[];
  runs: LiveWorkflowRunRecord[];
}
export interface LiveWorkflowSecretItem {
  name: string;
  updated_at: string;
}
export interface TriggerWorkflowRunInput {
  expected_commit: string;
  source_ref: string;
  workflow_name?: string;
}

const getCsrfHeader = (token: string): string => {
  try {
    if (typeof window !== "undefined") {
      const s =
        window.localStorage.getItem("gitust_csrf_token") ??
        window.localStorage.getItem("gitust_csrf");
      if (s && s.trim()) return s.trim();
    }
  } catch {
    // Ignore storage read errors
  }
  return `csrf-${token.slice(0, 16)}`;
};

const isObj = (v: unknown): v is Record<string, unknown> =>
  Boolean(v && typeof v === "object" && !Array.isArray(v));
const isStr = (v: unknown): v is string => typeof v === "string";

const parseArtifact = (raw: unknown): LiveWorkflowArtifact => {
  if (!isObj(raw) || !isStr(raw.name) || !isStr(raw.path) || typeof raw.size_bytes !== "number")
    throw new Error("Invalid workflow artifact entry");
  return {
    name: raw.name,
    path: raw.path,
    size_bytes: raw.size_bytes,
    content_preview: isStr(raw.content_preview) ? raw.content_preview : null,
  };
};

const parseRun = (raw: unknown): LiveWorkflowRunRecord => {
  if (!isObj(raw) || !isStr(raw.run_id) || !isStr(raw.repo_id) || !isStr(raw.workflow_name) || !isStr(raw.commit_sha) || !isStr(raw.source_ref) || (raw.status !== "passed" && raw.status !== "failed") || !Array.isArray(raw.jobs) || !isStr(raw.created_at))
    throw new Error("Invalid workflow run record");
  return {
    run_id: raw.run_id,
    repo_id: raw.repo_id,
    workflow_name: raw.workflow_name,
    commit_sha: raw.commit_sha,
    source_ref: raw.source_ref,
    status: raw.status,
    created_at: raw.created_at,
    jobs: raw.jobs.map((job) => {
      if (!isObj(job) || !isStr(job.job_id) || !isStr(job.display_name) || !Array.isArray(job.needs) || (job.status !== "passed" && job.status !== "failed" && job.status !== "skipped") || typeof job.cache_hit !== "boolean" || !Array.isArray(job.artifacts) || !isStr(job.redacted_output) || typeof job.duration_ms !== "number")
        throw new Error("Invalid workflow job run");
      const matrixValues: Record<string, string> = {};
      if (isObj(job.matrix_values)) {
        for (const [k, v] of Object.entries(job.matrix_values)) if (isStr(v)) matrixValues[k] = v;
      }
      return {
        job_id: job.job_id,
        display_name: job.display_name,
        needs: job.needs.filter(isStr),
        matrix_values: matrixValues,
        runner_id: isStr(job.runner_id) ? job.runner_id : null,
        isolation_tier: job.isolation_tier === "container_vm" ? "container_vm" : "host_os",
        status: job.status,
        cache_hit: job.cache_hit,
        artifacts: job.artifacts.map(parseArtifact),
        redacted_output: job.redacted_output,
        duration_ms: job.duration_ms,
      };
    }),
  };
};

const parseOverview = (raw: unknown): LiveRepositoryWorkflowsOverview => {
  if (!isObj(raw) || !Array.isArray(raw.workflows) || !Array.isArray(raw.runs))
    throw new Error("Invalid workflow overview payload");
  return {
    workflows: raw.workflows.map((wf) => {
      if (!isObj(wf) || !isStr(wf.name) || !Array.isArray(wf.on) || !Array.isArray(wf.jobs))
        throw new Error("Invalid workflow definition");
      return {
        name: wf.name,
        on: wf.on.filter(isStr),
        jobs: wf.jobs.map((j) => {
          if (!isObj(j) || !isStr(j.id) || !isStr(j.name) || !Array.isArray(j.needs))
            throw new Error("Invalid workflow job definition");
          return {
            id: j.id,
            name: j.name,
            needs: j.needs.filter(isStr),
            runs_on: isStr(j.runs_on) ? j.runs_on : "local",
            isolation_tier: j.isolation_tier === "container_vm" ? "container_vm" : "host_os",
            matrix: isObj(j.matrix) ? (j.matrix as Record<string, string[]>) : null,
            secrets: Array.isArray(j.secrets) ? j.secrets.filter(isStr) : [],
            cache_key: isStr(j.cache_key) ? j.cache_key : null,
            artifact_name: isStr(j.artifact_name) ? j.artifact_name : null,
            steps_count: typeof j.steps_count === "number" ? j.steps_count : 0,
          };
        }),
      };
    }),
    runs: raw.runs.map(parseRun),
  };
};

async function wfRequest<T>(path: string, parse: (v: unknown) => T, label: string, init?: { method?: string; body?: unknown; signal?: AbortSignal }): Promise<T> {
  const token = getLiveToken();
  if (!token) throw new Error("Sign in as SiteAdmin to manage workflows");
  const method = init?.method ?? "GET";
  const headers: Record<string, string> = { Authorization: `Bearer ${token}` };
  if (method !== "GET") {
    headers["Content-Type"] = "application/json";
    headers["X-CSRF-Token"] = getCsrfHeader(token);
  }
  const res = await fetch(path, { method, headers, body: init?.body !== undefined ? JSON.stringify(init.body) : undefined, cache: method === "GET" ? "no-store" : undefined, signal: init?.signal });
  if (!res.ok) {
    const detail = (await res.text().catch(() => "")).trim();
    throw new Error(`Unable to ${label} (HTTP ${res.status}${detail ? `: ${detail}` : ""})`);
  }
  return res.status === 204 ? parse(undefined) : parse(await res.json());
}

const repoUrl = (id: string, suffix: string) => `/api/v1/repos/${encodeURIComponent(id)}/${suffix}`;
export const fetchRepositoryWorkflows = (id: string, signal?: AbortSignal) =>
  wfRequest(repoUrl(id, "workflows"), parseOverview, "load workflows", { signal });
export const triggerWorkflowRun = (id: string, input: TriggerWorkflowRunInput) =>
  wfRequest(repoUrl(id, "workflow-runs"), parseRun, "trigger workflow run", { method: "POST", body: input });
export const fetchWorkflowArtifacts = (id: string, runId: string, signal?: AbortSignal) =>
  wfRequest(repoUrl(id, `workflow-runs/${encodeURIComponent(runId)}/artifacts`), (raw) => {
    if (!isObj(raw) || !Array.isArray(raw.artifacts)) throw new Error("Invalid workflow artifacts payload");
    return raw.artifacts.map(parseArtifact);
  }, "load workflow artifacts", { signal });
export const fetchWorkflowSecrets = (id: string, signal?: AbortSignal) =>
  wfRequest(repoUrl(id, "workflow-secrets"), (raw) => {
    if (!isObj(raw) || !Array.isArray(raw.secrets)) throw new Error("Invalid workflow secrets payload");
    return raw.secrets.map((s) => {
      if (!isObj(s) || !isStr(s.name) || !isStr(s.updated_at)) throw new Error("Invalid workflow secret item");
      return { name: s.name, updated_at: s.updated_at };
    });
  }, "load workflow secrets", { signal });
export const putWorkflowSecret = (id: string, name: string, value: string) =>
  wfRequest(repoUrl(id, `workflow-secrets/${encodeURIComponent(name)}`), (raw) => {
    if (!isObj(raw) || !isStr(raw.name) || !isStr(raw.updated_at)) throw new Error("Invalid workflow secret response");
    return { name: raw.name, updated_at: raw.updated_at };
  }, "save workflow secret", { method: "PUT", body: { value } });
export const deleteWorkflowSecret = (id: string, name: string) =>
  wfRequest(repoUrl(id, `workflow-secrets/${encodeURIComponent(name)}`), () => undefined, "delete workflow secret", { method: "DELETE" });
