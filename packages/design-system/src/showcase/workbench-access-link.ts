/** Consume link secrets before rendering, keeping them out of subsequent navigation URLs. */
export const readWorkbenchAccessLink = (): {
  mode: "invite" | "reset";
  token: string;
} | null => {
  if (typeof window === "undefined") return null;
  const path = window.location.pathname;
  const token = (
    new URLSearchParams(window.location.search).get("token") ?? ""
  ).trim();
  const mode =
    path.startsWith("/auth/accept-invite") || token.startsWith("gitust_inv_")
      ? "invite"
      : path.startsWith("/auth/reset-password") ||
          token.startsWith("gitust_rst_")
        ? "reset"
        : null;
  if (!mode) return null;
  window.history.replaceState({}, "", path);
  return { mode, token };
};
