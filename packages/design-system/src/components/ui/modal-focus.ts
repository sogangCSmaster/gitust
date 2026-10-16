type AutoFocusHandlers = {
  onOpenAutoFocus?: (event: Event) => void;
  onCloseAutoFocus?: (event: Event) => void;
};

/** Kobalte has no triggerRef for controlled dialogs opened by a separate workspace action. */
export function createModalFocusRestore(
  props: AutoFocusHandlers,
  modal: () => boolean,
) {
  let returnFocus: HTMLElement | null = null;
  return {
    onOpenAutoFocus(event: Event) {
      returnFocus =
        typeof document !== "undefined" &&
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
      props.onOpenAutoFocus?.(event);
    },
    onCloseAutoFocus(event: Event) {
      props.onCloseAutoFocus?.(event);
      if (modal() && !event.defaultPrevented && returnFocus?.isConnected) {
        returnFocus.focus({ preventScroll: true });
        event.preventDefault();
      }
    },
  };
}
