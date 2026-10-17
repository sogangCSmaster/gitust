# Browse branches without changing repository state

The Files and Changes views can browse a local branch in either the primary
workbench repository or a configured read-only repository. This does not switch a
checkout, change the default branch or grant verification/approval authority.
Tags and remote-tracking refs are outside this branch list. Managed `agent/*`
branches have the same repository read permissions as other local branches;
worktree review remains a separate workflow.

## Web selection

The compact branch control shows the selected commit's short SHA. A branch option
identifies a full `refs/heads/...` ref and the commit returned by the server. Files,
commit history, file contents and diffs use that full commit SHA. A later push
does not silently move a selected snapshot. Refreshing the branch list updates
its options; select the new tip to inspect it. The default option uses the
repository catalog's identified default-branch commit.

An unborn default branch does not mean the whole repository is empty. The branch
control remains available so a first push to `develop`, for example, can be
selected before `main` exists. Without a default commit or another selected
branch, the view retains its empty-default guidance.

Only a bounded page of branch options and the current selected snapshot are
retained. Next/First navigate the list; an option being absent from a page is not
evidence that the branch was deleted or access was revoked. Repository and account
changes invalidate in-flight responses. Existing worktree comparisons retain
their own head/base SHAs and are not replaced by branch-list refreshes.
On mobile, changing the file context returns to the list so the new branch's
files can be selected. Refreshing an unchanged selected path preserves its detail.

## Read API and limits

`GET /api/v1/repos/{configured-repository-uuid}/branches?limit=50` returns:

```json
{
  "branches": [
    {
      "name": "develop",
      "ref_name": "refs/heads/develop",
      "commit_sha": "0123456789abcdef0123456789abcdef01234567"
    }
  ],
  "next_cursor": null
}
```

The SHA above is an illustrative schema value. The default page size is 50 and
the maximum is 200. Results use byte order of the canonical full ref name. Pass
the returned opaque `next_cursor` unchanged as the URL-encoded `cursor` parameter;
do not construct cursors from display labels. The versioned unpadded base64url
cursor keeps supported UTF-8 ref names within the HTTP URI size limit.

The API permits names up to 32 KiB of UTF-8, scans at most 10,000 local heads,
retains at most `limit + 1` candidates and caps serialized JSON at 1 MiB.
A response may contain fewer entries than the requested limit
when its byte budget is reached; follow its cursor rather than inferring completion
from the row count. Each page is a live read, not a transaction spanning later
page requests. Reload from the first page to discover branches inserted earlier
in the ordering.

An actually empty branch namespace returns `branches: []` with a null cursor.
Invalid cursors, unsupported/corrupt ref state and exhausted safety bounds return
errors rather than a fabricated empty page. Reads require current repository
access and reauthenticate the original credential after Git work. Responses use
`Cache-Control: no-store`. There are no branch mutations or new transport routes.

## Verification

The existing empty-repository operating rehearsal checks UUID branch pages
against independent `git for-each-ref` output with a one-entry page size. It
covers empty primary/secondary repositories, a first alternate-branch push while
the default is unborn, the first default push and service restart. A maximum-size
opaque cursor also passes through the actual HTTP listener. Source-specific
unit/runtime and browser evidence is recorded separately from this contract.
