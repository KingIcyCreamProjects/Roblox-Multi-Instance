import * as Dialog from "@radix-ui/react-dialog";
import * as Toggle from "@radix-ui/react-toggle-group";
import { keepPreviousData, useMutation, useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Eye, Grid2X2, X } from "lucide-react";
import { createContext, useContext, useState, type ReactNode } from "react";
import { toast } from "@/components/Toasts";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { SCRIM, useModalFocus } from "@/components/ui/Sheet";
import { useSetSettings, useSettings } from "@/app/useSettings";
import { Avatar } from "@/features/accounts/AccountRow";
import { label } from "@/features/accounts/model";
import { useAccounts } from "@/features/accounts/useAccounts";
import {
  call,
  errMessage,
  type AccountView,
  type GridArrangeDefault,
  type GridLayout,
} from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { layoutLabel, sameLayout, tiles, togglePick } from "./model";

const role = (i: number) => (i === 0 ? t("grid.main") : t("grid.alt", { n: i }));

// ---- Step 1 ----

function PickOrder({
  accounts,
  running,
  order,
  setOrder,
  remembered,
  onNext,
}: {
  accounts: AccountView[];
  running: Set<string>;
  order: string[];
  /** an order is already saved */
  remembered: boolean;
  setOrder: (f: (o: string[]) => string[]) => void;
  onNext: () => void;
}) {
  // running first (sort is stable: the list order holds inside each half)
  const list = [...accounts].sort((a, b) => Number(running.has(b.id)) - Number(running.has(a.id)));
  return (
    <>
      <ul className="grid max-h-[50vh] grid-cols-2 gap-x-3 gap-y-1.5 overflow-auto p-1.5 -m-1.5">
        {list.map((a) => {
          const i = order.indexOf(a.id);
          const on = i >= 0;
          const run = running.has(a.id);
          return (
            <li key={a.id}>
              <button
                type="button"
                aria-pressed={on}
                onClick={() => setOrder((o) => togglePick(o, a.id))}
                className={clsx(
                  "flex h-13 w-full items-center gap-3 rounded-panel px-3.5 text-left transition-colors duration-[var(--dur-quick)]",
                  on ? "bg-surface-4" : "bg-surface-2 hover:bg-surface-3",
                )}
              >
                <span className="relative shrink-0">
                  <Avatar a={a} size={30} round />
                  {run && (
                    <span
                      aria-hidden
                      className="absolute -right-px -bottom-px size-2 rounded-full bg-online ring-2 ring-surface-4"
                    />
                  )}
                  {on && (
                    <span
                      aria-hidden
                      className="absolute -top-1.5 -right-1.5 grid size-5 place-items-center rounded-full bg-accent text-12 font-bold text-accent-ink tnum ring-2 ring-surface-4"
                    >
                      {i + 1}
                    </span>
                  )}
                </span>
                <span data-pii className="min-w-0 flex-1 truncate text-13 font-medium text-text-1">
                  {label(a)}
                </span>
                <span className="shrink-0 text-12 text-text-3">
                  {on ? role(i) : run ? t("grid.running") : ""}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
      <div className="flex items-center gap-2.5">
        <Button variant="ghost" onClick={() => setOrder(() => [])} disabled={!order.length}>
          {t("grid.clear")}
        </Button>
        {remembered && <span className="text-12 text-text-3">{t("grid.remembered")}</span>}
        <span className="flex-1" />
        <Button variant="primary" onClick={onNext} disabled={!order.length}>
          {t("grid.next")}
        </Button>
      </div>
    </>
  );
}

// ---- Step 2 ----

/** Layout preview drawn as a dark monitor in both themes (its tile text is --on-image). */
function Thumb({ layout, n }: { layout: GridLayout; n: number }) {
  const cas = layout.type === "cascade" && n > 1;
  return (
    <div
      aria-hidden
      className="relative h-35 overflow-hidden rounded-control bg-[rgb(40_42_50)] shadow-[inset_0_0_0_1px_rgb(255_255_255/0.08)]"
    >
      <div className="absolute inset-1.5">
        {tiles(layout, n).map((k) => {
          const main = k.i === 0;
          const big = main && k.w * k.h > 0.3;
          return (
            <div
              key={k.i}
              className="absolute p-[1.5px]"
              style={{
                left: `${k.x * 100}%`,
                top: `${k.y * 100}%`,
                width: `${k.w * 100}%`,
                height: `${k.h * 100}%`,
              }}
            >
              <div
                className={clsx(
                  "flex size-full items-center justify-center overflow-hidden rounded-chip leading-[1.15] shadow-[inset_0_0_0_1px_rgb(255_255_255/0.2)]",
                  cas ? "items-start justify-start gap-1.5 px-1.5 py-[3px]" : "flex-col",
                  main
                    ? "bg-accent/30 shadow-[inset_0_0_0_1px_rgb(120_160_255/0.6)]"
                    : "bg-white/15",
                  cas && (main ? "bg-[rgb(52_82_150)]" : "bg-[rgb(70_72_80/0.95)]"),
                )}
              >
                <b
                  className={clsx(
                    "font-semibold text-on-image tnum",
                    big ? "text-28" : cas ? "text-12" : "text-14",
                  )}
                >
                  {k.i + 1}
                </b>
                {n < 10 && (
                  <span className={clsx("text-on-image-2", "text-12", cas && "pt-px")}>
                    {role(k.i)}
                  </span>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function PickLayout({
  order,
  count,
  saved,
  onDone,
}: {
  order: string[];
  count: number;
  saved: GridArrangeDefault | undefined;
  onDone: () => void;
}) {
  const setSettings = useSetSettings();
  const {
    data: suggestions = [],
    error: suggestErr,
    isPlaceholderData,
  } = useQuery({
    queryKey: ["grid_suggest", count],
    queryFn: () => call("grid_suggest", { count }),
    placeholderData: keepPreviousData, // no blank flash when the window count settles
  });
  const {
    data: monitors = [],
    isSuccess: monitorsReady,
    error: monitorsErr,
  } = useQuery({
    queryKey: ["monitors"],
    queryFn: () => call("monitors_list"),
    // displays come and go (dock, undock): re-read on each open, like the windows
    refetchOnMount: "always",
  });
  // the layout itself, not an index: the suggestion list changes when the count settles
  const [picked, setPicked] = useState<GridLayout | null>(null);
  const [mon, setMon] = useState(saved?.monitor ?? null);
  // the picked layout, else the saved one when still on offer, else the best one
  const find = (l: GridLayout | null | undefined) =>
    suggestions.findIndex((s) => sameLayout(s.layout, l ?? undefined));
  const pickedIdx = find(picked);
  const idx = pickedIdx >= 0 ? pickedIdx : Math.max(0, find(saved?.layout));
  const several = monitors.length > 1;
  const monitor = several
    ? (monitors.find((m) => m.id === mon) ?? monitors.find((m) => m.primary) ?? monitors[0]).id
    : undefined;
  const layout = suggestions[idx]?.layout;

  const arrange = useMutation({
    mutationFn: async () => {
      const n = await call("grid_arrange", { monitor, order, layout });
      setSettings.mutate({ gridArrange: { ...(monitor ? { monitor } : {}), order, layout } });
      return n;
    },
    onSuccess: (n) => {
      toast(n === 1 ? t("grid.done1") : n ? t("grid.done", { n }) : t("grid.empty"));
      onDone();
    },
    onError: (e) => toast(errMessage(e)),
  });
  const identify = () =>
    void call("grid_identify", { order }).then(
      (n) => n || toast(t("grid.empty")),
      (e) => toast(errMessage(e)),
    );

  return (
    <>
      {/* Radix ToggleGroup: radio roles, one Tab stop, arrow keys (as ServerPick) */}
      <Toggle.Root
        type="single"
        value={String(idx)}
        onValueChange={(v) => v && setPicked(suggestions[Number(v)].layout)}
        aria-label={t("grid.layoutTitle")}
        className="grid grid-cols-3 gap-3.5"
      >
        {suggestions.map((s, i) => (
          <Toggle.Item
            key={s.label}
            value={String(i)}
            className={clsx(
              "flex flex-col gap-3 rounded-panel px-3 pt-3 pb-3.5 text-left transition-colors duration-[var(--dur-quick)]",
              i === idx
                ? "bg-surface-4 shadow-[inset_0_0_0_1.5px_var(--accent)]"
                : "bg-surface-2 hover:bg-surface-3",
            )}
          >
            <Thumb layout={s.layout} n={count} />
            <span className="flex items-center justify-between px-0.5">
              <span className="text-13 font-semibold text-text-1">{layoutLabel(s, count)}</span>
              {i === 0 && (
                <span className="rounded-full bg-accent px-2.5 py-0.5 text-12 font-semibold text-accent-ink">
                  {t("grid.best")}
                </span>
              )}
            </span>
          </Toggle.Item>
        ))}
      </Toggle.Root>
      {(suggestErr ?? monitorsErr) && (
        <p role="status" className="text-12 text-error">
          {errMessage(suggestErr ?? monitorsErr)}
        </p>
      )}
      <div className="flex items-center gap-2.5">
        {several && (
          // same segmented recipe as ServerPick
          <Toggle.Root
            type="single"
            value={monitor ?? ""}
            onValueChange={(v) => v && setMon(v)}
            aria-label={t("grid.monitor")}
            className="flex h-8 items-center rounded-control bg-surface-3 p-0.5"
          >
            {monitors.map((m) => (
              <Toggle.Item
                key={m.id}
                value={m.id}
                className="h-7 rounded-[calc(var(--radius-control)-2px)] px-3 text-12 font-semibold text-text-2 transition-colors duration-[var(--dur-quick)] hover:text-text-1 data-[state=on]:bg-surface-4 data-[state=on]:text-text-1"
              >
                {m.label}
              </Toggle.Item>
            ))}
          </Toggle.Root>
        )}
        <span className="flex-1" />
        <IconButton label={t("grid.identify")} onClick={identify}>
          <Eye size={16} strokeWidth={1.75} />
        </IconButton>
        <Button
          variant="primary"
          onClick={() => arrange.mutate()}
          // before monitors load, the saved monitor would be overwritten with null
          // placeholder suggestions were built for the previous window count
          disabled={!layout || !monitorsReady || isPlaceholderData || arrange.isPending}
        >
          <Grid2X2 size={16} strokeWidth={1.75} aria-hidden />
          {t("grid.go")}
        </Button>
      </div>
    </>
  );
}

// ---- Popover ----

/** One focus handler set per open (the loading shell and the wizard are separate Contents). */
const Focus = createContext<ReturnType<typeof useModalFocus> | null>(null);

function Shell({ wide, children }: { wide: boolean; children: ReactNode }) {
  const focus = useContext(Focus);
  return (
    <Dialog.Content
      {...focus}
      style={{ width: `min(${wide ? 53.75 : 43.75}rem, calc(100vw - 48px))` }}
      className="glass fixed top-1/2 left-1/2 z-50 flex max-h-[calc(100vh-48px)] overflow-y-auto -translate-x-1/2 -translate-y-1/2 flex-col gap-5 rounded-panel p-6"
    >
      {children}
    </Dialog.Content>
  );
}

function Wizard({
  accounts,
  saved,
  onClose,
}: {
  accounts: AccountView[];
  saved: GridArrangeDefault | undefined;
  onClose: () => void;
}) {
  const setSettings = useSetSettings();
  const [order, setOrder] = useState(() => {
    const ids = new Set(accounts.map((a) => a.id));
    return (saved?.order ?? []).filter((id) => ids.has(id));
  });
  const [step, setStep] = useState<1 | 2>(order.length ? 2 : 1);
  // refetched on every open (Body mounts per open) and every 2 s while open: presence lags,
  // windows are the truth, and a client still loading (busy) or launched meanwhile only counts
  // once its window answers
  const { data: win } = useQuery({
    queryKey: ["grid_windows"],
    queryFn: () => call("grid_windows"),
    refetchOnMount: "always",
    refetchInterval: 2000,
  });
  const running = new Set(win?.accounts);
  // grid_arrange tiles the picked accounts that have a window (in order), then windows of no
  // account; unpicked accounts' windows stay put
  const live = order.filter((id) => running.has(id)).length + (win?.unknown ?? 0);
  const count = live || order.length;
  return (
    <Shell wide={step === 2}>
      <div className="flex items-start justify-between">
        <div>
          <Dialog.Title className="text-20 font-semibold tracking-[-0.01em] text-text-1">
            {step === 1 ? t("grid.pickTitle") : t("grid.layoutTitle")}
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-13 text-text-2">
            {step === 1 ? (
              t("grid.pickSub")
            ) : (
              <>
                {order.length === 1
                  ? t("grid.subOne")
                  : order.length === 2
                    ? t("grid.subTwo")
                    : t("grid.sub", { n: order.length, alts: order.length - 1 })}{" "}
                {live > 0 && live !== order.length && <>{t("grid.openNow", { n: live })} </>}
                <button
                  type="button"
                  onClick={() => setStep(1)}
                  className="underline underline-offset-[3px] hover:text-text-1"
                >
                  {t("grid.editOrder")}
                </button>
              </>
            )}
          </Dialog.Description>
        </div>
        <span className="flex items-start gap-3">
          <span className="pt-1 text-12 text-text-3">{t("grid.step", { n: step })}</span>
          {/* same close control as Sheet */}
          <Dialog.Close
            aria-label={t("sheet.close")}
            className="-mt-1 -mr-2 inline-grid size-8 place-items-center rounded-control text-text-2 hover:bg-surface-3 hover:text-text-1"
          >
            <X size={16} strokeWidth={1.75} />
          </Dialog.Close>
        </span>
      </div>
      {step === 1 ? (
        <PickOrder
          accounts={accounts}
          running={running}
          order={order}
          setOrder={setOrder}
          remembered={!!saved?.order?.length}
          onNext={() => {
            setSettings.mutate({ gridArrange: { order } });
            setStep(2);
          }}
        />
      ) : (
        <PickLayout order={order} count={count} saved={saved} onDone={onClose} />
      )}
    </Shell>
  );
}

export function GridPopover({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
}) {
  const focus = useModalFocus();
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className={SCRIM} />
        <Focus.Provider value={focus}>
          <Body onClose={() => onOpenChange(false)} />
        </Focus.Provider>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** Mounted only while open: a fresh start (step, picks) each time. Waits for the saved order. */
function Body({ onClose }: { onClose: () => void }) {
  const accounts = useAccounts().data?.accounts;
  const { data: settings } = useSettings();
  if (!accounts || !settings) {
    return (
      <Shell wide={false}>
        <Dialog.Title className="sr-only">{t("grid.arrange")}</Dialog.Title>
        <Dialog.Description className="sr-only">{t("grid.arrange")}</Dialog.Description>
        <div className="h-40" />
      </Shell>
    );
  }
  return (
    <Wizard
      accounts={accounts}
      saved={settings.gridArrange as GridArrangeDefault | undefined}
      onClose={onClose}
    />
  );
}
