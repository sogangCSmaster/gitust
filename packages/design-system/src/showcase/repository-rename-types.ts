export interface RepositoryRenameIntent {
  repoId: string;
  name: string;
  token: string;
  epoch: number;
  key: number;
}
export type RepositoryRenameResult =
  { kind: "saved" } | { kind: "stale" } | { kind: "error"; error: string };
export interface RepositoryRenameActions {
  begin: () => RepositoryRenameIntent | null;
  isCurrent: (intent: RepositoryRenameIntent) => boolean;
  save: (
    intent: RepositoryRenameIntent,
    name: string,
  ) => Promise<RepositoryRenameResult>;
  cancel: (intent: RepositoryRenameIntent) => void;
}

export const validRepositoryName = (name: string) =>
  name.length > 0 &&
  name.length <= 100 &&
  /^[A-Za-z0-9._-]+$/.test(name) &&
  !name.startsWith(".") &&
  !name.endsWith(".") &&
  !name.includes("..");
