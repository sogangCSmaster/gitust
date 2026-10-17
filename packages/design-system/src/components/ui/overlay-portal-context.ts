import { createContext, type Accessor } from "solid-js";

// A nested popup must stay inside its modal's accessibility and focus boundary.
export const OverlayPortalMountContext =
  createContext<Accessor<HTMLElement | undefined>>();
