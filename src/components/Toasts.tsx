import clsx from "clsx";
import { X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { create } from "zustand";
import { t } from "@/lib/i18n";

type Toast = {
  id: number;
  message: string;
  ms: number;
  action?: { label: string; run: () => void };
};
/** `locked`: no Toaster is mounted (App sets it), so `toast()` drops instead of queueing a burst for unlock. */
type Q = { items: Toast[]; locked: boolean; dismiss: (id: number) => void };

let next = 1;
export const useToasts = create<Q>((set) => ({
  items: [],
  locked: false,
  dismiss: (id) => set((s) => ({ items: s.items.filter((x) => x.id !== id) })),
}));

/** Queue a toast. Default 4 s, 8 s when it carries an action. Returns its dismiss. Dropped while locked. */
export function toast(message: string, action?: Toast["action"], ms = action ? 8000 : 4000) {
  if (useToasts.getState().locked) return () => {};
  return heldToast(message, action, ms);
}

/** `toast()` that is queued even while locked, shown once the Shell's Toaster mounts. */
export function heldToast(message: string, action?: Toast["action"], ms = action ? 8000 : 4000) {
  const id = next++;
  useToasts.setState((s) => {
    const items = [...s.items, { id, message, ms, action }];
    // ponytail: at most 3 plain and 3 action (Undo) toasts, oldest evicted first; separate caps
    // so a burst of plain toasts never pushes out an Undo
    const oldest = (xs: Toast[]) => xs.slice(0, Math.max(0, xs.length - 3));
    const drop = new Set(
      [...oldest(items.filter((x) => !x.action)), ...oldest(items.filter((x) => x.action))].map(
        (x) => x.id,
      ),
    );
    return { items: items.filter((x) => !drop.has(x.id)) };
  });
  return () => useToasts.getState().dismiss(id);
}

/** One toast; its timer pauses while hovered or while focus is inside it. */
function Item({ x, dismiss }: { x: Toast; dismiss: (id: number) => void }) {
  const [hover, setHover] = useState(false);
  const [focus, setFocus] = useState(false);
  const [left, setLeft] = useState(x.ms);
  const paused = hover || focus;
  useEffect(() => {
    if (paused) return;
    const t0 = Date.now();
    const timer = setTimeout(() => dismiss(x.id), left);
    return () => {
      clearTimeout(timer);
      setLeft((l) => l - (Date.now() - t0));
    };
    // `left` is read only when (re)starting the timer
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paused, dismiss, x.id]);
  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: 4 }}
      transition={{ duration: 0.16, ease: [0.16, 1, 0.3, 1] }}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onFocus={() => setFocus(true)}
      onBlur={(e) => !e.currentTarget.contains(e.relatedTarget) && setFocus(false)}
      className="pointer-events-auto flex min-h-10 w-full items-center gap-3 glass rounded-panel py-1.5 pr-1.5 pl-3.5 text-13 text-text-1"
    >
      <p className="min-w-0 flex-1 break-words">{x.message}</p>
      {x.action && (
        <button
          type="button"
          className="rounded-control px-2.5 py-1 text-13 font-medium text-accent-text hover:bg-surface-4"
          onClick={() => {
            x.action!.run();
            dismiss(x.id);
          }}
        >
          {x.action.label}
        </button>
      )}
      <button
        type="button"
        aria-label={t("toast.dismiss")}
        className="inline-grid size-7 place-items-center rounded-control text-text-3 hover:bg-surface-4 hover:text-text-1"
        onClick={() => dismiss(x.id)}
      >
        <X size={14} strokeWidth={1.75} />
      </button>
    </motion.div>
  );
}

/** Bottom-right, above the Play bar (`playbar`) or else above a page's 4rem footer (Settings save
 * bar). The region is a polite live region, so toasts are announced. */
export function Toaster({ playbar = true }: { playbar?: boolean }) {
  const { items, dismiss } = useToasts();
  // Portaled to body so it is not trapped under dialog scrims by .app-win's stacking context.
  return createPortal(
    <div
      role="status"
      aria-live="polite"
      aria-label={t("toast.region")}
      className={clsx(
        "pointer-events-none fixed right-7 z-[60] flex w-[21.25rem] max-w-[calc(100vw-3.5rem)] flex-col items-end gap-2",
        playbar ? "bottom-[calc(var(--playbar-h)+12px)]" : "bottom-[calc(4rem+12px)]",
      )}
    >
      <AnimatePresence initial={false}>
        {items.map((x) => (
          <Item key={x.id} x={x} dismiss={dismiss} />
        ))}
      </AnimatePresence>
    </div>,
    document.body,
  );
}
