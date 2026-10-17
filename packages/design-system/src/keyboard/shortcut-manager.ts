import { createSignal } from "solid-js";

export interface ShortcutDefinition {
  /** Unique identifier, e.g. "nav.issues" */
  id: string;
  /** Human-readable title shown in Command Palette */
  title: string;
  /** Category for grouping in Command Palette & Help Modal */
  category:
    "Navigation" | "Repository" | "Issues & PRs" | "View & Theme" | "Actions";
  /**
   * Keybinding pattern:
   * - Single key: "c", "/", "?"
   * - Modifier combo: "mod+k", "mod+shift+p"
   * - Chord sequence (space-separated): "g i", "g p", "g c"
   */
  keys: string;
  /** Current workspace capability; disabled commands are hidden and cannot be dispatched. */
  enabled?: () => boolean;
  /** Handler invoked when matched */
  handler: (event: KeyboardEvent) => void;
  /** Optional subtitle or description */
  description?: string;
  /** Whether this shortcut should work even when focused inside an input/textarea */
  allowInInput?: boolean;
  /** Whether holding the key down (key repeat) should repeatedly invoke the handler (e.g. j/k navigation) */
  allowRepeat?: boolean;
  /** Whether this shortcut is allowed to fire while a native or ARIA modal dialog is active */
  allowWhenModalOpen?: boolean;
}

export interface KeystrokeTelemetry {
  keys: string;
  title: string;
  dispatchLatencyMs: number;
  timestamp: number;
}

const CHORD_TIMEOUT_MS = 900;

const [activeChordSignal, setActiveChordSignal] = createSignal<string | null>(
  null,
);
const [lastTelemetrySignal, setLastTelemetrySignal] =
  createSignal<KeystrokeTelemetry | null>(null);
const [shortcutsListSignal, setShortcutsListSignal] = createSignal<
  ShortcutDefinition[]
>([]);

class ShortcutManager {
  private registry = new Map<string, ShortcutDefinition>();
  private keyMap = new Map<string, ShortcutDefinition>();
  private chordPrefixes = new Set<string>();

  private pendingChordPrefix: string | null = null;
  private chordTimer: ReturnType<typeof setTimeout> | null = null;

  private attached = false;

  public get activeChord() {
    return activeChordSignal;
  }

  public get lastTelemetry() {
    return lastTelemetrySignal;
  }

  public get shortcuts() {
    return shortcutsListSignal;
  }

  public register(def: ShortcutDefinition): () => void {
    const normalizedKeys = this.normalizePattern(def.keys);
    const normalizedDef: ShortcutDefinition = {
      ...def,
      keys: normalizedKeys,
    };

    this.registry.set(def.id, normalizedDef);
    this.rebuildIndex();

    if (typeof window !== "undefined" && !this.attached) {
      this.attach();
    }

    return () => {
      this.registry.delete(def.id);
      this.rebuildIndex();
    };
  }

  public executeById(id: string, syntheticEvent?: KeyboardEvent): boolean {
    const start = performance.now();
    const def = this.registry.get(id);
    if (!def || def.enabled?.() === false) return false;

    const ev =
      syntheticEvent ??
      new KeyboardEvent("keydown", { key: def.keys, bubbles: true });
    def.handler(ev);
    const elapsed = performance.now() - start;

    setLastTelemetrySignal({
      keys: def.keys,
      title: def.title,
      dispatchLatencyMs: Number(elapsed.toFixed(3)),
      timestamp: Date.now(),
    });
    return true;
  }

  private rebuildIndex(): void {
    this.keyMap.clear();
    this.chordPrefixes.clear();

    const list: ShortcutDefinition[] = [];
    for (const def of this.registry.values()) {
      list.push(def);
      this.keyMap.set(def.keys, def);
      const parts = def.keys.split(" ");
      if (parts.length > 1 && parts[0]) {
        this.chordPrefixes.add(parts[0]);
      }
    }
    setShortcutsListSignal(list);
  }

  private normalizePattern(pattern: string): string {
    return pattern.trim().toLowerCase().split(/\s+/).join(" ");
  }

  private eventToToken(event: KeyboardEvent): string | null {
    const rawKey = event.key;
    if (
      rawKey === "Meta" ||
      rawKey === "Control" ||
      rawKey === "Alt" ||
      rawKey === "Shift"
    ) {
      return null;
    }

    const parts: string[] = [];
    const hasMod = event.metaKey || event.ctrlKey;
    if (hasMod) parts.push("mod");
    if (event.altKey) parts.push("alt");

    const isAsciiLetter =
      rawKey.length === 1 &&
      ((rawKey >= "a" && rawKey <= "z") || (rawKey >= "A" && rawKey <= "Z"));
    if (
      event.shiftKey &&
      (rawKey.length > 1 || hasMod || event.altKey || isAsciiLetter)
    ) {
      parts.push("shift");
    }

    const normalizedKey = rawKey === " " ? "space" : rawKey.toLowerCase();
    parts.push(normalizedKey);
    return parts.join("+");
  }

  private isEditableElement(target: EventTarget | null): boolean {
    if (!(target instanceof HTMLElement)) return false;
    const tag = target.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
      return true;
    }
    return (
      target.isContentEditable ||
      target.closest(
        '[role="combobox"], [role="listbox"], [role="menu"], [role="tree"], [role="grid"], [aria-haspopup="listbox"], [aria-haspopup="menu"]',
      ) !== null
    );
  }

  private isModalDialogOpen(): boolean {
    if (typeof document === "undefined") return false;
    return (
      document.querySelector(
        'dialog[open], [role="dialog"][aria-modal="true"]:not([data-closed]), [role="alertdialog"][aria-modal="true"]:not([data-closed])',
      ) !== null
    );
  }

  private clearChord(): void {
    if (this.chordTimer) {
      clearTimeout(this.chordTimer);
      this.chordTimer = null;
    }
    this.pendingChordPrefix = null;
    setActiveChordSignal(null);
  }

  public attach(): void {
    if (this.attached || typeof window === "undefined") return;
    this.attached = true;
    window.addEventListener("keydown", this.handleKeyDown, { capture: true });
  }

  public detach(): void {
    if (!this.attached || typeof window === "undefined") return;
    this.clearChord();
    this.attached = false;
    window.removeEventListener("keydown", this.handleKeyDown, {
      capture: true,
    });
  }

  private handleKeyDown = (event: KeyboardEvent): void => {
    const start = performance.now();
    const token = this.eventToToken(event);
    if (!token) return;

    if (token === "escape" && this.pendingChordPrefix) {
      this.clearChord();
      return;
    }

    const inInput = this.isEditableElement(event.target);
    const modalOpen = this.isModalDialogOpen();

    if (inInput || modalOpen) {
      if (this.pendingChordPrefix) {
        this.clearChord();
      }
    }

    // 1. If a chord prefix is currently active (e.g., user pressed 'g' earlier)
    if (this.pendingChordPrefix && !inInput && !modalOpen) {
      if (event.repeat) return;
      const chordCandidate = `${this.pendingChordPrefix} ${token}`;
      const matchedChord = this.keyMap.get(chordCandidate);
      this.clearChord();

      if (matchedChord && matchedChord.enabled?.() !== false) {
        event.preventDefault();
        event.stopPropagation();
        matchedChord.handler(event);
        const elapsed = performance.now() - start;
        setLastTelemetrySignal({
          keys: matchedChord.keys,
          title: matchedChord.title,
          dispatchLatencyMs: Number(elapsed.toFixed(3)),
          timestamp: Date.now(),
        });
        return;
      }
    }

    // 2. Check if this single token starts a chord sequence (only when not typing in an input or modal)
    if (!inInput && !modalOpen && this.chordPrefixes.has(token)) {
      if (event.repeat) return;
      event.preventDefault();
      this.pendingChordPrefix = token;
      setActiveChordSignal(token);
      if (this.chordTimer) clearTimeout(this.chordTimer);
      this.chordTimer = setTimeout(() => {
        this.clearChord();
      }, CHORD_TIMEOUT_MS);
      return;
    }

    // 3. Direct single-key or modifier shortcut match
    const directMatch = this.keyMap.get(token);
    if (directMatch && directMatch.enabled?.() !== false) {
      if (event.repeat && !directMatch.allowRepeat) {
        return;
      }
      if (modalOpen && !directMatch.allowWhenModalOpen) {
        return;
      }
      if (inInput && !directMatch.allowInInput && !token.startsWith("mod+")) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      directMatch.handler(event);
      const elapsed = performance.now() - start;
      setLastTelemetrySignal({
        keys: directMatch.keys,
        title: directMatch.title,
        dispatchLatencyMs: Number(elapsed.toFixed(3)),
        timestamp: Date.now(),
      });
    }
  };
}

export const shortcutManager = new ShortcutManager();

/**
 * Formats a shortcut string (e.g., "mod+k" or "g i") into display tokens
 * like ["⌘", "K"] or ["G", "then", "I"].
 */
export function formatShortcutKeys(keys: string): string[] {
  const isMac =
    typeof navigator !== "undefined" &&
    /Mac|iPod|iPhone|iPad/.test(navigator.platform);

  const chordParts = keys.trim().split(/\s+/);
  if (chordParts.length > 1) {
    return chordParts.map((part) => part.toUpperCase());
  }

  const single = chordParts[0] ?? "";
  return single.split("+").map((token) => {
    switch (token.toLowerCase()) {
      case "mod":
        return isMac ? "⌘" : "Ctrl";
      case "shift":
        return "⇧";
      case "alt":
        return isMac ? "⌥" : "Alt";
      case "enter":
        return "↵";
      case "escape":
      case "esc":
        return "Esc";
      case "arrowup":
        return "↑";
      case "arrowdown":
        return "↓";
      default:
        return token.toUpperCase();
    }
  });
}
