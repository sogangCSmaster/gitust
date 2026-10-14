import { Component, createEffect, JSX, onCleanup, onMount } from "solid-js";
import { Kbd } from "./Kbd";

export interface DialogProps {
  open: boolean;
  onClose: () => void;
  title: string;
  children: JSX.Element;
  class?: string;
}

/**
 * Modern HTML <dialog> modal adhering to Baseline best practices:
 * - Opens via showModal() for top-layer promotion and focus trapping
 * - Uses declarative closedby="any" with Safari coordinate light-dismiss fallback
 * - Uses CSS overscroll-behavior: contain on dialog:modal & dialog::backdrop
 */
export const Dialog: Component<DialogProps> = (props) => {
  let dialogEl: HTMLDialogElement | undefined;

  createEffect(() => {
    if (!dialogEl) return;
    if (props.open && !dialogEl.open) {
      dialogEl.showModal();
    } else if (!props.open && dialogEl.open) {
      dialogEl.close();
    }
  });

  onMount(() => {
    if (!dialogEl) return;

    const handleClose = () => {
      if (props.open) props.onClose();
    };
    dialogEl.addEventListener("close", handleClose);

    // Fallback for browsers without native <dialog closedby="any"> support (e.g. Safari)
    const handleBackdropClick = (event: MouseEvent) => {
      if (!dialogEl || "closedBy" in HTMLDialogElement.prototype) return;
      if (event.target !== dialogEl) return;

      const rect = dialogEl.getBoundingClientRect();
      const isDialogContent =
        rect.top <= event.clientY &&
        event.clientY <= rect.top + rect.height &&
        rect.left <= event.clientX &&
        event.clientX <= rect.left + rect.width;

      if (!isDialogContent) {
        dialogEl.close();
      }
    };

    dialogEl.addEventListener("click", handleBackdropClick);

    onCleanup(() => {
      dialogEl?.removeEventListener("close", handleClose);
      dialogEl?.removeEventListener("click", handleBackdropClick);
    });
  });

  return (
    <dialog
      ref={dialogEl}
      closedby="any"
      aria-label={props.title}
      class={`ds-dialog ${props.class ?? ""}`}
    >
      <div class="ds-dialog-header">
        <h2 class="ds-dialog-title">{props.title}</h2>
        <button
          type="button"
          class="ds-btn ds-btn--ghost ds-btn--xs"
          onClick={() => props.onClose()}
        >
          <Kbd keys="esc" />
        </button>
      </div>
      <div class="ds-dialog-body">{props.children}</div>
    </dialog>
  );
};
