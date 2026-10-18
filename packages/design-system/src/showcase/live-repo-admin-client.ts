import { getLiveToken } from "./live-repo-context";

export interface LiveRepositorySettings {
  id: string;
  name: string;
  description: string | null;
  default_branch: string;
  changeable_branches: string[];
  is_private: boolean;
  archived: boolean;
  git_write: boolean;
  workbench_enabled: boolean;
  is_default: boolean;
}

export interface UpdateRepositorySettingsPatch {
  name?: string;
  description?: string | null;
  default_branch?: string;
  is_private?: boolean;
  archived?: boolean;
  workbench_enabled?: boolean;
}

export interface CreateHostedRepositoryInput {
  name: string;
  owner_team_id: string;
  is_private: boolean;
  default_branch?: string;
  description?: string;
  workbench_enabled?: boolean;
}

const getCsrfHeader = (token: string): string => {
  try {
    if (typeof window !== "undefined") {
      const stored =
        window.localStorage.getItem("gitust_csrf_token") ??
        window.localStorage.getItem("gitust_csrf");
      if (stored && stored.trim()) return stored.trim();
    }
  } catch {
    // Ignore storage read errors in restricted environments
  }
  return `csrf-${token.slice(0, 16)}`;
};

const parseSettings = (raw: unknown): LiveRepositorySettings => {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("Invalid repository settings payload");
  }
  const obj = raw as Record<string, unknown>;
  if (
    typeof obj.id !== "string" ||
    typeof obj.name !== "string" ||
    (obj.description !== null &&
      obj.description !== undefined &&
      typeof obj.description !== "string") ||
    typeof obj.default_branch !== "string"
  ) {
    throw new Error("Malformed repository settings fields");
  }
  return {
    id: obj.id,
    name: obj.name,
    description: typeof obj.description === "string" ? obj.description : null,
    default_branch: obj.default_branch,
    changeable_branches: Array.isArray(obj.changeable_branches)
      ? obj.changeable_branches.filter(
          (b): b is string => typeof b === "string",
        )
      : [obj.default_branch],
    is_private: Boolean(obj.is_private),
    archived: Boolean(obj.archived),
    git_write: Boolean(obj.git_write),
    workbench_enabled: Boolean(obj.workbench_enabled),
    is_default: Boolean(obj.is_default),
  };
};

const requireToken = (): string => {
  const token = getLiveToken();
  if (!token) {
    throw new Error("Sign in as SiteAdmin to administer repositories");
  }
  return token;
};

export async function fetchRepositorySettings(
  repoId: string,
  signal?: AbortSignal,
): Promise<LiveRepositorySettings> {
  const token = requireToken();
  const res = await fetch(
    `/api/v1/repos/${encodeURIComponent(repoId)}/settings`,
    {
      headers: { Authorization: `Bearer ${token}` },
      cache: "no-store",
      signal,
    },
  );
  if (!res.ok) {
    const detail = (await res.text().catch(() => "")).trim();
    throw new Error(
      `Unable to load repository settings (HTTP ${res.status}${detail ? `: ${detail}` : ""})`,
    );
  }
  return parseSettings(await res.json());
}

export async function updateRepositorySettings(
  repoId: string,
  patch: UpdateRepositorySettingsPatch,
): Promise<LiveRepositorySettings> {
  const token = requireToken();
  const res = await fetch(
    `/api/v1/repos/${encodeURIComponent(repoId)}/settings`,
    {
      method: "PATCH",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
        "X-CSRF-Token": getCsrfHeader(token),
      },
      body: JSON.stringify(patch),
    },
  );
  if (!res.ok) {
    const detail = (await res.text().catch(() => "")).trim();
    throw new Error(
      `Unable to update repository settings (HTTP ${res.status}${detail ? `: ${detail}` : ""})`,
    );
  }
  return parseSettings(await res.json());
}

export async function createHostedRepository(
  input: CreateHostedRepositoryInput,
): Promise<LiveRepositorySettings> {
  const token = requireToken();
  const res = await fetch("/api/v1/admin/repos", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
      "X-CSRF-Token": getCsrfHeader(token),
    },
    body: JSON.stringify(input),
  });
  if (!res.ok) {
    const detail = (await res.text().catch(() => "")).trim();
    throw new Error(
      `Unable to provision repository (HTTP ${res.status}${detail ? `: ${detail}` : ""})`,
    );
  }
  return parseSettings(await res.json());
}
