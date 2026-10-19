// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IconSpriteSheet } from "../src/components/workspace/icon";
import {
  fetchRepositorySearch,
  type LiveRepositorySearchReport,
} from "../src/showcase/live-code-search-client";
import {
  RepositoryFilesView,
  type RepositoryFileEntry,
} from "../src/showcase/RepositoryFilesView";

const sampleFiles: RepositoryFileEntry[] = [
  {
    path: "src/auth.rs",
    kind: "file",
    content: [
      "use crate::error::AuthError;",
      "pub struct Principal {",
      "    pub id: String,",
      "}",
      "pub fn verify_token(raw: &str) -> Result<Principal, AuthError> {",
      "    todo!()",
      "}",
    ].join("\n"),
  },
  {
    path: "src/lib.rs",
    kind: "file",
    content: [
      "pub mod auth;",
      "pub trait TokenVerifier {",
      "    fn check(&self, token: &str) -> bool;",
      "}",
    ].join("\n"),
  },
];

describe("Slice 10 (Phase 4.5) — Indexed Repository-Authorized Code & Symbol Search", () => {
  beforeEach(() => {
    window.localStorage.setItem("gitust_token", "test-live-session-token");
    vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: vi.fn(),
    });
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    window.localStorage.clear();
  });

  it("searches live repository code and symbols via GET /api/v1/repos/{id}/search and navigates to the matched file and line", async () => {
    const mockReport: LiveRepositorySearchReport = {
      revision: "main",
      commitSha: "a1b2c3d4e5f60123456789abcdef0123456789ab",
      query: "verify_token",
      kind: "symbol",
      indexedFileCount: 12,
      indexCacheHit: true,
      hits: [
        {
          path: "src/auth.rs",
          lineNumber: 42,
          snippet:
            "pub fn verify_token(raw: &str) -> Result<Principal, AuthError>",
          matchKind: "symbol",
          symbolName: "verify_token",
          symbolKind: "function",
        },
      ],
    };

    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input.toString();
      if (
        url ===
        "/api/v1/repos/11111111-1111-4111-8111-111111111111/search?q=verify_token&kind=symbol&rev=a1b2c3d4e5f60123456789abcdef0123456789ab"
      ) {
        return new Response(JSON.stringify(mockReport), { status: 200 });
      }
      return new Response("Not Found", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const onSelect = vi.fn();
    render(() => (
      <>
        <IconSpriteSheet />
        <RepositoryFilesView
          files={sampleFiles}
          liveMode={true}
          repositoryId="11111111-1111-4111-8111-111111111111"
          revision="a1b2c3d4e5f60123456789abcdef0123456789ab"
          onSelect={onSelect}
        />
      </>
    ));

    // Select the Symbols filter pill
    fireEvent.click(screen.getByRole("button", { name: "Symbols" }));

    // Enter query and submit
    const searchInput = screen.getByLabelText(
      "Search repository code and symbols",
    );
    fireEvent.input(searchInput, { target: { value: "verify_token" } });
    fireEvent.click(screen.getByRole("button", { name: /^Search$/i }));

    await waitFor(() => {
      expect(screen.getByText(/a1b2c3d4e5f6/)).toBeTruthy();
      expect(screen.getByText(/12 files indexed/)).toBeTruthy();
      expect(screen.getByText("src/auth.rs:42")).toBeTruthy();
      expect(screen.getByText("function")).toBeTruthy();
    });

    // Click the hit to navigate to the file and line
    fireEvent.click(screen.getByText("src/auth.rs:42"));

    await waitFor(() => {
      expect(onSelect).toHaveBeenCalledWith(
        expect.objectContaining({ path: "src/auth.rs", kind: "file" }),
      );
      expect(screen.getByText("Line 42")).toBeTruthy();
    });
  });

  it("supports deterministic local symbol and code search over committed file entries when liveMode is false", async () => {
    render(() => (
      <>
        <IconSpriteSheet />
        <RepositoryFilesView files={sampleFiles} liveMode={false} />
      </>
    ));

    fireEvent.click(screen.getByRole("button", { name: "Symbols" }));
    const searchInput = screen.getByLabelText(
      "Search repository code and symbols",
    );
    fireEvent.input(searchInput, { target: { value: "TokenVerifier" } });
    fireEvent.click(screen.getByRole("button", { name: /^Search$/i }));

    await waitFor(() => {
      expect(screen.getByText("src/lib.rs:2")).toBeTruthy();
      expect(screen.getByText("trait")).toBeTruthy();
    });

    fireEvent.click(screen.getByText("src/lib.rs:2"));

    await waitFor(() => {
      expect(screen.getByText("Line 2")).toBeTruthy();
    });
  });

  it("validates live-code-search-client response payloads fail-closed", async () => {
    const badShaFetcher = vi.fn(async () => {
      return new Response(
        JSON.stringify({
          revision: "main",
          commitSha: "not-a-valid-hex-sha!",
          query: "fn",
          kind: "all",
          indexedFileCount: 1,
          indexCacheHit: false,
          hits: [],
        }),
        { status: 200 },
      );
    }) as unknown as typeof fetch;

    await expect(
      fetchRepositorySearch("11111111-1111-4111-8111-111111111111", "fn", {
        fetcher: badShaFetcher,
      }),
    ).rejects.toThrow(/Invalid repository search response/i);

    const badHitFetcher = vi.fn(async () => {
      return new Response(
        JSON.stringify({
          revision: "main",
          commitSha: "a1b2c3d4e5f60123456789abcdef0123456789ab",
          query: "fn",
          kind: "all",
          indexedFileCount: 1,
          indexCacheHit: false,
          hits: [
            {
              path: "src/lib.rs",
              lineNumber: 0,
              snippet: "bad line number",
              matchKind: "code",
            },
          ],
        }),
        { status: 200 },
      );
    }) as unknown as typeof fetch;

    await expect(
      fetchRepositorySearch("11111111-1111-4111-8111-111111111111", "fn", {
        fetcher: badHitFetcher,
      }),
    ).rejects.toThrow(/Invalid search hit record/i);
  });
});
