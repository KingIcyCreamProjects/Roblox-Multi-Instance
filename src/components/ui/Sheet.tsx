import * as Dialog from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import { useRef, type ReactNode } from "react";
import { t } from "@/lib/i18n";

/** Scrim of the modal dialogs. It starts below the title bar so the frameless window can still
 * be dragged, minimized, maximized and closed (see `.titlebar` in index.css). */
export const SCRIM =
  "sheet-fade fixed inset-x-0 top-[var(--titlebar-h)] bottom-0 z-40 bg-[var(--scrim)]";

/** Dialog.Content props for a dialog opened from store state (no Dialog.Trigger, so Radix would
 * drop focus on body): focus goes back to the opener on close, and pressing the title bar or a
 * toast (`data-modal-pass`) does not close the dialog. */
export function useModalFocus() {
  const opener = useRef<HTMLElement | null>(null);
  return {
    onOpenAutoFocus: () => {
      opener.current = document.activeElement as HTMLElement | null;
    },
    onCloseAutoFocus: (e: Event) => {
      if (!opener.current?.isConnected) return;
      e.preventDefault();
      opener.current.focus({ preventScroll: true });
    },
    onInteractOutside: (e: Event) => {
      if (e.target instanceof Element && e.target.closest("[data-modal-pass]")) e.preventDefault();
    },
  };
}

/** Right-side sheet (Radix Dialog): focus is trapped, Esc and the scrim close it. */
export function Sheet({
  open,
  onOpenChange,
  title,
  description,
  width = 440,
  centered = false,
  children,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  title: string;
  description?: string;
  width?: number;
  /** Centered pop-up instead of right-side sheet. */
  centered?: boolean;
  children: ReactNode;
}) {
  const focus = useModalFocus();
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className={SCRIM} />
        <Dialog.Content
          {...focus}
          // no description: the title alone names it (a hidden copy would be read twice)
          {...(description ? {} : { "aria-describedby": undefined })}
          className={
            centered
              ? "sheet-pop fixed top-1/2 left-1/2 glass z-50 flex max-h-[calc(100vh-48px)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-panel"
              : "sheet-slide fixed top-[var(--titlebar-h)] right-3 bottom-3 glass z-50 flex flex-col rounded-panel"
          }
          style={{
            // rem, so the UI scale grows the sheet with its text (`width` is px at 100%)
            width: centered
              ? `min(${width / 16}rem, calc(100vw - 48px))`
              : `min(${width / 16}rem, 100vw)`,
          }}
        >
          <header className="flex items-start gap-3 px-6 pt-6 pb-4">
            <div className="flex-1">
              <Dialog.Title className="text-20 font-semibold tracking-[-0.01em] text-text-1">
                {title}
              </Dialog.Title>
              {description && (
                <Dialog.Description className="mt-1 text-13 text-text-2">
                  {description}
                </Dialog.Description>
              )}
            </div>
            <Dialog.Close
              aria-label={t("sheet.close")}
              className="-mt-1 -mr-2 inline-grid size-8 place-items-center rounded-control text-text-2 hover:bg-surface-3 hover:text-text-1"
            >
              <X size={16} strokeWidth={1.75} />
            </Dialog.Close>
          </header>
          <div className="min-h-0 flex-1 overflow-auto px-6 pb-6">{children}</div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
