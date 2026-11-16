import * as Dialog from "@radix-ui/react-dialog";
import { keepPreviousData, useMutation, useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Eye, Grid2X2 } from "lucide-react";
import { useState, type ReactNode } from "react";
import { toast } from "@/components/Toasts";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
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
import { sameLayout, tiles, togglePick } from "./model";

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
                  "flex h-[52px] w-full items-center gap-3 rounded-[10px] px-3.5 text-left transition-colors duration-[var(--dur-quick)]",
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
                      className="absolute -top-1.5 -right-1.5 grid size-[19px] place-items-center rounded-full bg-accent text-[11px] font-bold text-accent-ink tnum ring-2 ring-surface-4"
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
      className="relative h-[140px] overflow-hidden rounded-[8px] bg-[rgb(40_42_50)] shadow-[inset_0_0_0_1px_rgb(255_255_255/0.08)]"
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
                  "flex size-full items-center justify-center overflow-hidden rounded-[4px] leading-[1.15] shadow-[inset_0_0_0_1px_rgb(255_255_255/0.2)]",
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
                    big ? "text-[30px]" : cas ? "text-[11px]" : "text-[15px]",
                  )}
                >
                  {k.i + 1}
                </b>
                {n < 10 && (
                  <span
                    className={clsx(
                      "text-on-image-2",
                      big ? "text-[11.5px]" : "text-[9.5px]",
                      cas && "pt-px",
                    )}
                  >
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
      <div
        role="radiogroup"
        aria-label={t("grid.layoutTitle")}
        className="grid grid-cols-3 gap-3.5"
      >
        {suggestions.map((s, i) => (
          <button
            key={s.label}
            type="button"
            role="radio"
            aria-checked={i === idx}
            onClick={() => setPicked(s.layout)}
            className={clsx(
              "flex flex-col gap-3 rounded-[12px] px-3 pt-3 pb-3.5 text-left transition-colors duration-[var(--dur-quick)]",
              i === idx
                ? "bg-surface-4 shadow-[inset_0_0_0_1.5px_var(--accent)]"
                : "bg-surface-2 hover:bg-surface-3",
            )}
          >
            <Thumb layout={s.layout} n={count} />
            <span className="flex items-center justify-between px-0.5">
              <span className="text-13 font-semibold text-text-1">{s.label}</span>
              {i === 0 && (
                <span className="rounded-full bg-accent px-2.5 py-[3px] text-[11.5px] font-semibold text-accent-ink">
                  {t("grid.best")}
                </span>
              )}
            </span>
          </button>
        ))}
      </div>
      {(suggestErr ?? monitorsErr) && (
        <p role="status" className="text-12 text-error">
          {errMessage(suggestErr ?? monitorsErr)}
        </p>
      )}
      <div className="flex items-center gap-2.5">
        {several && (
          <div
            role="radiogroup"
            aria-label={t("grid.monitor")}
            className="flex gap-0.5 rounded-[9px] bg-surface-3 p-[3px]"
          >
            {monitors.map((m) => (
              <button
                key={m.id}
                type="button"
                role="radio"
                aria-checked={m.id === monitor}
                onClick={() => setMon(m.id)}
                className={clsx(
                  "h-[26px] rounded-[7px] px-3 text-12 font-semibold transition-colors duration-[var(--dur-quick)]",
                  m.id === monitor ? "bg-surface-4 text-text-1" : "text-text-2 hover:text-text-1",
                )}
              >
                {m.label}
              </button>
            ))}
          </div>
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

function Shell({ wide, children }: { wide: boolean; children: ReactNode }) {
  return (
    <Dialog.Content
      style={{ width: `min(${wide ? 860 : 700}px, calc(100vw - 48px))` }}
      className="glass fixed top-1/2 left-1/2 z-50 flex max-h-[calc(100vh-48px)] overflow-y-auto -translate-x-1/2 -translate-y-1/2 flex-col gap-[22px] rounded-[16px] p-7"
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
        <span className="pt-1 text-12 text-text-3">{t("grid.step", { n: step })}</span>
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
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="sheet-fade fixed inset-0 z-40 bg-[var(--scrim)]" />
        <Body onClose={() => onOpenChange(false)} />
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
