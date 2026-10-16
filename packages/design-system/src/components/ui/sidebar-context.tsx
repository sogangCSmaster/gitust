// SPDX-License-Identifier: MIT
// Adapted from solid-ui; see LICENSE.solid-ui and PROVENANCE.md.
import {
  createContext,
  createEffect,
  createSignal,
  mergeProps,
  onCleanup,
  splitProps,
  useContext,
  type Accessor,
  type Component,
  type ComponentProps,
  type JSX,
} from "solid-js";
import { cn } from "../../lib/utils";
const MOBILE_BREAKPOINT = 768;
const SIDEBAR_COOKIE_NAME = "sidebar:state";
const SIDEBAR_COOKIE_MAX_AGE = 60 * 60 * 24 * 7;
const SIDEBAR_WIDTH = "14rem";
const SIDEBAR_WIDTH_MOBILE = "18rem";
const SIDEBAR_WIDTH_ICON = "3rem";
const SIDEBAR_KEYBOARD_SHORTCUT = "b";

type SidebarContext = {
  state: Accessor<"expanded" | "collapsed">;
  open: Accessor<boolean>;
  setOpen: (open: boolean) => void;
  openMobile: Accessor<boolean>;
  setOpenMobile: (open: boolean) => void;
  isMobile: Accessor<boolean>;
  toggleSidebar: () => void;
};

const SidebarContext = createContext<SidebarContext | null>(null);

function useSidebar() {
  const context = useContext(SidebarContext);
  if (!context) {
    throw new Error("useSidebar must be used within a Sidebar.");
  }

  return context;
}

function useIsMobile(fallback = false) {
  const [isMobile, setIsMobile] = createSignal(fallback);

  createEffect(() => {
    const mql = window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT - 1}px)`);
    const onChange = (e: MediaQueryListEvent | MediaQueryList) => {
      setIsMobile(e.matches);
    };
    mql.addEventListener("change", onChange);
    onChange(mql);
    onCleanup(() => mql.removeEventListener("change", onChange));
  });

  return isMobile;
}

type SidebarProviderProps = Omit<ComponentProps<"div">, "style"> & {
  defaultOpen?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  style?: JSX.CSSProperties;
};

const SidebarProvider: Component<SidebarProviderProps> = (rawProps) => {
  const props = mergeProps({ defaultOpen: true }, rawProps);
  const [local, others] = splitProps(props, [
    "defaultOpen",
    "open",
    "onOpenChange",
    "class",
    "style",
    "children",
  ]);

  const isMobile = useIsMobile();
  const [openMobile, setOpenMobile] = createSignal(false);

  // This is the internal state of the sidebar.
  // We use open and onOpenChange for control from outside the component.
  const [_open, _setOpen] = createSignal(local.defaultOpen);
  const open = () => local.open ?? _open();
  const setOpen = (value: boolean | ((value: boolean) => boolean)) => {
    if (local.onOpenChange) {
      return local.onOpenChange?.(
        typeof value === "function" ? value(open()) : value,
      );
    }
    _setOpen(value);

    // This sets the cookie to keep the sidebar state.
    document.cookie = `${SIDEBAR_COOKIE_NAME}=${open()}; path=/; max-age=${SIDEBAR_COOKIE_MAX_AGE}`;
  };

  // Helper to toggle the sidebar.
  const toggleSidebar = () => {
    return isMobile()
      ? setOpenMobile((open) => !open)
      : setOpen((open) => !open);
  };

  // Adds a keyboard shortcut to toggle the sidebar.
  createEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (
        event.key === SIDEBAR_KEYBOARD_SHORTCUT &&
        (event.metaKey || event.ctrlKey)
      ) {
        event.preventDefault();
        toggleSidebar();
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    onCleanup(() => window.removeEventListener("keydown", handleKeyDown));
  });

  // We add a state so that we can do data-state="expanded" or "collapsed".
  // This makes it easier to style the sidebar with Tailwind classes.
  const state = () => (open() ? "expanded" : "collapsed");

  const contextValue = {
    state,
    open,
    setOpen,
    isMobile,
    openMobile,
    setOpenMobile,
    toggleSidebar,
  };

  return (
    <SidebarContext.Provider value={contextValue}>
      <div
        style={{
          "--sidebar-width": SIDEBAR_WIDTH,
          "--sidebar-width-icon": SIDEBAR_WIDTH_ICON,
          ...local.style,
        }}
        class={cn(
          "group/sidebar-wrapper flex min-h-svh w-full text-sidebar-foreground has-[[data-variant=inset]]:bg-sidebar",
          local.class,
        )}
        {...others}
      >
        {local.children}
      </div>
    </SidebarContext.Provider>
  );
};

export { SidebarProvider, useSidebar, SIDEBAR_WIDTH_MOBILE };
