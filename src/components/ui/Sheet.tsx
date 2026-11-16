import * as Dialog from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import type { ReactNode } from "react";
import { t } from "@/lib/i18n";

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
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="sheet-fade fixed inset-0 z-40 bg-[var(--scrim)]" />
        <Dialog.Content
          className={
            centered
              ? "sheet-pop fixed top-1/2 left-1/2 glass z-50 flex max-h-[calc(100vh-48px)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-panel"
              : "sheet-slide fixed top-[var(--titlebar-h)] right-3 bottom-3 glass z-50 flex flex-col rounded-panel"
          }
          style={{
            width: centered ? `min(${width}px, calc(100vw - 48px))` : `min(${width}px, 100vw)`,
          }}
        >
          <header className="flex items-start gap-3 px-6 pt-6 pb-4">
            <div className="flex-1">
              <Dialog.Title className="text-20 font-semibold tracking-[-0.01em] text-text-1">
                {title}
              </Dialog.Title>
              {description ? (
                <Dialog.Description className="mt-1 text-13 text-text-2">
                  {description}
                </Dialog.Description>
              ) : (
                <Dialog.Description className="sr-only">{title}</Dialog.Description>
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
