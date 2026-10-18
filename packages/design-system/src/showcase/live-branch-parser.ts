import type { RepositoryBranch } from "./live-branches";

export interface BranchPage {
  branches: RepositoryBranch[];
  next_cursor: string | null;
}
const bytes = (value: string) => new TextEncoder().encode(value).length;
const invalid = () => new Error("Server returned invalid repository branches");
const ref = (value: unknown): value is string =>
  typeof value === "string" &&
  value.startsWith("refs/heads/") &&
  bytes(value.slice(11)) > 0 &&
  bytes(value.slice(11)) <= 32768 &&
  !/[\x00-\x20\x7f~^:?*\[\\]/u.test(value) &&
  !value.includes("..") &&
  !value.includes("@{") &&
  value
    .split("/")
    .every(
      (part) => part && !part.startsWith(".") && !part.endsWith(".lock"),
    ) &&
  !value.endsWith(".");
const cursorRef = (value: string) => {
  try {
    const encoded = value.slice(3);
    const binary = atob(encoded.replace(/-/g, "+").replace(/_/g, "/"));
    if (
      btoa(binary)
        .replace(/\+/g, "-")
        .replace(/\//g, "_")
        .replace(/=+$/, "") !== encoded
    )
      throw invalid();
    const name = new TextDecoder("utf-8", { fatal: true }).decode(
      Uint8Array.from(binary, (char) => char.charCodeAt(0)),
    );
    if (!ref(name)) throw invalid();
    return name;
  } catch {
    throw invalid();
  }
};
export const parseBranchPage = (raw: unknown): BranchPage => {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) throw invalid();
  const page = raw as Record<string, unknown>;
  if (
    Object.keys(page).sort().join() !== "branches,next_cursor" ||
    !Array.isArray(page.branches) ||
    page.branches.length > 200 ||
    !(
      page.next_cursor === null ||
      (typeof page.next_cursor === "string" &&
        /^v1\.[A-Za-z0-9_-]+$/.test(page.next_cursor) &&
        page.next_cursor.length <= 43709)
    )
  )
    throw invalid();
  let previous: Uint8Array | undefined;
  const branches = page.branches.map((value): RepositoryBranch => {
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw invalid();
    const row = value as Record<string, unknown>;
    if (
      Object.keys(row).sort().join() !== "commit_sha,name,ref_name" ||
      !ref(row.ref_name) ||
      typeof row.name !== "string" ||
      row.ref_name !== `refs/heads/${row.name}` ||
      typeof row.commit_sha !== "string" ||
      !/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(row.commit_sha) ||
      /^0+$/.test(row.commit_sha)
    )
      throw invalid();
    const current = new TextEncoder().encode(row.ref_name);
    if (previous) {
      let index = 0;
      while (
        index < Math.min(previous.length, current.length) &&
        previous[index] === current[index]
      )
        index++;
      if (
        index === current.length ||
        (index < previous.length && previous[index]! > current[index]!)
      )
        throw invalid();
    }
    previous = current;
    return {
      name: row.name,
      ref_name: row.ref_name,
      commit_sha: row.commit_sha.toLowerCase(),
    };
  });
  if (
    page.next_cursor !== null &&
    (!branches.length ||
      cursorRef(page.next_cursor as string) !== branches.at(-1)?.ref_name)
  )
    throw invalid();
  return { branches, next_cursor: page.next_cursor as string | null };
};

export async function boundedBranchJson(response: Response) {
  const limit = 4 * 1024 * 1024;
  if (Number(response.headers.get("content-length")) > limit) throw invalid();
  const reader = response.body?.getReader();
  if (!reader) throw invalid();
  let size = 0;
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let text = "";
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > limit) throw invalid();
      text += decoder.decode(value, { stream: true });
    }
    text += decoder.decode();
    return JSON.parse(text) as unknown;
  } finally {
    await reader.cancel();
  }
}
