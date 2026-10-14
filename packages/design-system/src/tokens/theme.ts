import { createSignal } from "solid-js";

export type ThemeMode = "dark" | "light";

const [themeSignal, setThemeSignal] = createSignal<ThemeMode>("dark");

export function getTheme(): ThemeMode {
  return themeSignal();
}

export function setTheme(mode: ThemeMode): void {
  setThemeSignal(mode);
  if (typeof document !== "undefined") {
    document.documentElement.setAttribute("data-theme", mode);
  }
}

export function toggleTheme(): ThemeMode {
  const next: ThemeMode = themeSignal() === "dark" ? "light" : "dark";
  setTheme(next);
  return next;
}
