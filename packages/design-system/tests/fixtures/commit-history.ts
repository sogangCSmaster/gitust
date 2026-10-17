/** A successful full-SHA history starts at that commit, including a root commit. */
export const commitHistory = (sha: string) => ({
  head_sha: sha,
  commits: [
    {
      sha,
      short_sha: sha.slice(0, 7),
      summary: "Fixture commit",
      author_name: "Fixture",
    },
  ],
});
