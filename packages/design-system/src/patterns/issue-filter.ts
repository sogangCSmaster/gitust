import type { IssueItem } from "./IssueListView";

export function filterIssueItems(
  items: IssueItem[],
  rawQuery: string,
): IssueItem[] {
  const q = rawQuery.trim().toLowerCase();
  if (!q) return items;
  return items.filter((item) => {
    if (q.includes("is:open") && item.status !== "open") return false;
    if (q.includes("is:closed") && item.status !== "closed") return false;
    if (q.includes("is:merged") && item.status !== "merged") return false;
    if (q.includes("is:pr") && item.kind !== "pr") return false;
    if (q.includes("is:issue") && item.kind !== "issue") return false;

    const cleanText = q.replace(/is:(open|closed|merged|pr|issue)/g, "").trim();
    if (!cleanText) return true;
    return (
      item.title.toLowerCase().includes(cleanText) ||
      item.author.toLowerCase().includes(cleanText) ||
      `#${item.id}`.includes(cleanText) ||
      item.labels.some((l) => l.name.toLowerCase().includes(cleanText))
    );
  });
}
