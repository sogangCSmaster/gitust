// @vitest-environment jsdom
import { createSignal, Show } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { EnterpriseCredentialsPanel } from "../src/patterns/EnterpriseCredentialsPanel";
import { RepositoryOverview } from "../src/showcase/RepositoryOverview";
import { createRepositoryWorkspace } from "../src/showcase/repository-workspace";

const repo = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "primary",
  branch: "main",
  head_commit: "a".repeat(40),
  capability: "workbench",
  is_default: true,
  is_private: true,
  can_change_visibility: true,
  can_invite_to_group: true,
};
const second = {
  ...repo,
  id: "22222222-2222-4222-8222-222222222222",
  name: "secondary",
  capability: "browse",
  is_default: false,
};
const team = "33333333-3333-4333-8333-333333333333";
const creator = "44444444-4444-4444-8444-444444444444";
const reply = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status });
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
};
const issued = (body: Record<string, string>) => ({
  repository_id: repo.id,
  owner_team_id: team,
  invite: {
    id: body.invite_id,
    purpose: "account_invite",
    target_email: body.target_email,
    assigned_role: body.assigned_role,
    created_by: creator,
    created_at: "2026-10-18T01:00:00.123456Z",
    expires_at: "2026-10-21T01:00:00.123456Z",
    used_at: null,
    revoked_at: null,
    team_ids: [team],
    repository_id: repo.id,
  },
  invite_url_path: `/auth/accept-invite?token=gitust_inv_${"a".repeat(64)}`,
});
beforeEach(() => {
  localStorage.setItem("gitust_token", "session-a");
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
const setup = async (
  post: (body: Record<string, string>, init: RequestInit) => Promise<Response>,
  hint: boolean | null = true,
) => {
  let rows = [repo, second].map((row) => {
    const { can_invite_to_group, ...old } = row;
    return hint === null ? old : { ...old, can_invite_to_group: hint };
  });
  const fetcher = vi.fn(async (_url: string, init?: RequestInit) =>
    init?.method === "POST"
      ? post(JSON.parse(String(init.body)), init)
      : reply({ repositories: rows }),
  );
  vi.stubGlobal("fetch", fetcher);
  const [shown, setShown] = createSignal(true);
  let workspace!: ReturnType<typeof createRepositoryWorkspace>;
  const hydrate = vi.fn(),
    clearView = vi.fn();
  render(() => {
    const [live] = createSignal(true);
    workspace = createRepositoryWorkspace({
      liveMode: live,
      setActiveTab: vi.fn(),
      hydrate,
      clearView,
    });
    return (
      <Show when={shown() && workspace.selected()}>
        {(selected) => (
          <RepositoryOverview
            repository={selected()}
            liveMode={live()}
            worktrees={1}
            actions={workspace}
            onNavigate={workspace.navigate}
          />
        )}
      </Show>
    );
  });
  await workspace.refresh();
  expect(workspace.state().repositories).toHaveLength(2);
  workspace.open(workspace.state().repositories[0]!);
  return {
    workspace,
    setShown,
    fetcher,
    hydrate,
    clearView,
    withdraw: () => {
      rows = rows.map((row) => ({ ...row, can_invite_to_group: false }));
    },
  };
};
const openAccess = async () => {
  const trigger = await screen.findByRole("button", {
    name: "Change repository access",
  });
  trigger.focus();
  await fireEvent.click(trigger);
  await vi.dynamicImportSettled();
  return trigger;
};
const openInvite = async () => {
  await openAccess();
  await fireEvent.click(
    screen.getByRole("button", { name: "Invite to owning group" }),
  );
  await vi.dynamicImportSettled();
  const email = await screen.findByRole("textbox", { name: "Email" });
  await waitFor(() =>
    expect(document.activeElement).toBe(
      (email as HTMLInputElement).disabled
        ? screen.getByRole("button", { name: "Close", exact: true })
        : email,
    ),
  );
  return email;
};
const submit = () =>
  fireEvent.submit(screen.getByRole("dialog").querySelector("form")!);
const posts = (fetcher: ReturnType<typeof vi.fn>) =>
  fetcher.mock.calls.filter(([, init]) => init?.method === "POST");

it("issues an owning-group invitation through access settings, clears its one-time link on close and keeps the original intent", async () => {
  const state = await setup(async (body) => reply(issued(body), 201));
  expect(
    screen.queryByRole("button", { name: "Invite to owning group" }),
  ).toBeNull();
  await fireEvent.input(await openInvite(), {
    target: { value: "  New.User@Example.test  " },
  });
  expect(screen.getByLabelText("Site-wide role").textContent).toContain(
    "Developer",
  );
  expect(screen.getByText(/other private repositories/i)).toBeTruthy();
  const create = screen.getByRole("button", { name: "Create invitation" });
  create.focus();
  await fireEvent.click(create);
  const link = (await screen.findByRole("textbox", {
    name: "Invitation link",
  })) as HTMLInputElement;
  expect(link.readOnly).toBe(true);
  await waitFor(() => expect(document.activeElement).toBe(link));
  expect(link.value).toBe(
    `${window.location.origin}${issued({}).invite_url_path}`,
  );
  expect(localStorage.getItem("gitust_token")).toBe("session-a");
  expect(Object.values(localStorage).join()).not.toContain("gitust_inv_");
  const call = posts(state.fetcher)[0]!;
  expect(call[0]).toBe(`/api/v1/repos/${repo.id}/group-invites`);
  expect(call[1].headers.Authorization).toBe("Bearer session-a");
  const request = JSON.parse(call[1].body);
  expect(request).toMatchObject({
    target_email: "new.user@example.test",
    assigned_role: "Developer",
  });
  expect(request.invite_id).toMatch(/^[0-9a-f-]{36}$/);
  await fireEvent.keyDown(link, { key: "Escape", code: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  state.setShown(false);
  state.setShown(true);
  await openInvite();
  expect(screen.queryByRole("textbox", { name: "Invitation link" })).toBeNull();
  expect(screen.getByText(/link is no longer shown/i)).toBeTruthy();
  expect(posts(state.fetcher)).toHaveLength(1);
  expect(state.hydrate).toHaveBeenCalledTimes(1);
  expect(state.clearView).toHaveBeenCalledTimes(1);
});

it("retries uncertain issuance with the same ID across close/reopen and creates a different intent only on explicit request", async () => {
  const pending = deferred<Response>();
  let count = 0;
  const state = await setup(async (body) => {
    count++;
    if (count === 1) return pending.promise;
    if (count === 2) {
      const { invite_url_path, ...metadata } = issued(body);
      return reply({ code: "invite_already_issued", ...metadata }, 409);
    }
    return reply(issued(body), 201);
  });
  await fireEvent.input(await openInvite(), {
    target: { value: "new@example.test" },
  });
  await submit();
  await submit();
  expect(posts(state.fetcher)).toHaveLength(1);
  expect(
    (
      screen.getByRole("button", {
        name: "Retry same invitation",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  const id = JSON.parse(posts(state.fetcher)[0]![1].body).invite_id;
  pending.resolve(reply({ code: "invite_unconfirmed", invite_id: id }, 503));
  await screen.findByRole("alert");
  await fireEvent.click(
    screen.getByRole("button", { name: "Close", exact: true }),
  );
  await openInvite();
  await submit();
  await screen.findByText(/already issued/i);
  expect(JSON.parse(posts(state.fetcher)[1]![1].body).invite_id).toBe(id);
  expect(screen.queryByRole("textbox", { name: "Invitation link" })).toBeNull();
  expect(screen.getByText(/does not revoke earlier links/i)).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "New invitation" }));
  await waitFor(() =>
    expect(document.activeElement).toBe(
      screen.getByRole("textbox", { name: "Email" }),
    ),
  );
  await submit();
  await screen.findByRole("textbox", { name: "Invitation link" });
  expect(JSON.parse(posts(state.fetcher)[2]![1].body).invite_id).not.toBe(id);
});

it.each(["selection", "session"])(
  "never displays a late link after %s replacement",
  async (change) => {
    let bodyResolve!: (value: string) => void;
    let request!: Record<string, string>;
    const state = await setup(async (body) => {
      request = body;
      return {
        status: 201,
        text: () =>
          new Promise<string>((done) => {
            bodyResolve = done;
          }),
      } as Response;
    });
    await fireEvent.input(await openInvite(), {
      target: { value: "new@example.test" },
    });
    await submit();
    await waitFor(() => expect(bodyResolve).toBeTypeOf("function"));
    if (change === "selection") {
      state.workspace.open(state.workspace.state().repositories[1]!);
      state.workspace.open(state.workspace.state().repositories[0]!);
    } else {
      localStorage.setItem("gitust_token", "session-b");
      window.dispatchEvent(
        new StorageEvent("storage", { key: "gitust_token" }),
      );
    }
    bodyResolve(JSON.stringify(issued(request)));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(
      screen.queryByRole("textbox", { name: "Invitation link" }),
    ).toBeNull();
    localStorage.setItem("gitust_token", "session-b");
    await state.workspace.refresh();
    state.workspace.open(state.workspace.state().repositories[0]!);
    await openInvite();
    expect(
      (screen.getByRole("textbox", { name: "Email" }) as HTMLInputElement)
        .value,
    ).toBe("");
    expect(posts(state.fetcher)).toHaveLength(1);
  },
);

it("requires a positive invite capability and clears links when it is withdrawn", async () => {
  const state = await setup(async (body) => reply(issued(body), 201));
  await fireEvent.input(await openInvite(), {
    target: { value: "new@example.test" },
  });
  await submit();
  await screen.findByRole("textbox", { name: "Invitation link" });
  state.withdraw();
  await state.workspace.refresh();
  await waitFor(() =>
    expect(
      screen.queryByRole("textbox", { name: "Invitation link" }),
    ).toBeNull(),
  );
  expect(
    screen.queryByRole("button", { name: "Invite to owning group" }),
  ).toBeNull();
  cleanup();
  await setup(async () => reply({}), null);
  await openAccess();
  expect(
    screen.queryByRole("button", { name: "Invite to owning group" }),
  ).toBeNull();
});

it.each(["repository", "group", "link"])(
  "withholds a %s-mismatched response and permits only the same-key retry",
  async (kind) => {
    const state = await setup(async (body) => {
      const result = issued(body);
      if (kind === "repository") result.invite.repository_id = second.id;
      if (kind === "group") result.invite.team_ids = [creator];
      if (kind === "link") result.invite_url_path += "&extra=1";
      return reply(result, 201);
    });
    await fireEvent.input(await openInvite(), {
      target: { value: "new@example.test" },
    });
    await submit();
    await screen.findByRole("alert");
    expect(
      screen.queryByRole("textbox", { name: "Invitation link" }),
    ).toBeNull();
    expect(
      screen.getByRole("button", { name: "Retry same invitation" }),
    ).toBeTruthy();
    expect(posts(state.fetcher)).toHaveLength(1);
  },
);

it("identifies persisted group grants in Access without inventing a scope for older invitations", async () => {
  const revoke = vi.fn();
  const item = {
    id: creator,
    purpose: "account_invite" as const,
    target_email: "new@example.test",
    assigned_role: "Developer" as const,
    expires_at: "2099-01-01T00:00:00Z",
    used_at: null,
    revoked_at: null,
  };
  render(() => (
    <EnterpriseCredentialsPanel
      view="invitations"
      allowedDomain="example.test"
      redeemMode="invite"
      onChangeRedeemMode={vi.fn()}
      redeemToken=""
      onRedeemTokenInput={vi.fn()}
      onPolicyFeedback={vi.fn()}
      onRevokeInvite={revoke}
      liveInvites={[
        { ...item, team_ids: [team], repository_id: repo.id },
        { ...item, id: repo.id, team_ids: [] },
        { ...item, id: second.id },
        {
          ...item,
          id: team,
          purpose: "password_reset",
          target_email: "existing@example.test",
          team_ids: [],
        },
      ]}
    />
  ));
  expect(screen.getByText("Owning group")).toBeTruthy();
  expect(screen.getAllByText("Account only")).toHaveLength(1);
  expect(screen.getByText("Password reset")).toBeTruthy();
  expect(screen.getByText("Existing account")).toBeTruthy();
  expect(screen.getByText("Scope unavailable")).toBeTruthy();
  await fireEvent.click(screen.getByText("Owning group"));
  expect(screen.getByText(`Invitation ID: ${creator}`)).toBeTruthy();
  expect(screen.getByText(`Repository: ${repo.id}`)).toBeTruthy();
  expect(screen.getByText(`Group: ${team}`)).toBeTruthy();
  await fireEvent.click(
    screen.getAllByRole("button", { name: "Revoke invite" })[0]!,
  );
  expect(revoke).toHaveBeenCalledWith(creator);
});

it("clears the selected target on denied issuance without showing private server details", async () => {
  const state = await setup(async () =>
    reply({ message: "private server detail" }, 403),
  );
  await fireEvent.input(await openInvite(), {
    target: { value: "new@example.test" },
  });
  await submit();
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(state.workspace.selected()).toBeNull();
  expect(screen.queryByText("private server detail")).toBeNull();
  expect(posts(state.fetcher)).toHaveLength(1);
});

it("submits supported dotless domains while enforcing the email UTF8 byte bound", async () => {
  const state = await setup(async () =>
    reply({ code: "invite_request_conflict" }, 409),
  );
  await fireEvent.input(await openInvite(), {
    target: { value: "NÄw@internal" },
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "Create invitation" }),
  );
  await waitFor(() => expect(posts(state.fetcher)).toHaveLength(1));
  expect(JSON.parse(posts(state.fetcher)[0]![1].body).target_email).toBe(
    "nÄw@internal",
  );
  await screen.findByRole("alert");
  await fireEvent.click(screen.getByRole("button", { name: "New invitation" }));
  await fireEvent.input(screen.getByRole("textbox", { name: "Email" }), {
    target: { value: `${"가".repeat(83)}@internal` },
  });
  await submit();
  expect(screen.getByRole("alert").textContent).toContain(
    "valid email address",
  );
  expect(posts(state.fetcher)).toHaveLength(1);
});
