import * as Tabs from "@radix-ui/react-tabs";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useUi } from "@/app/store";
import { Button } from "@/components/ui/Button";
import { Sheet } from "@/components/ui/Sheet";
import { Avatar } from "@/features/accounts/AccountRow";
import { label } from "@/features/accounts/model";
import { useAccounts } from "@/features/accounts/useAccounts";
import { call, type AccountView } from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { DisplayName, Password, inp } from "./panes";
import {
  Blocks,
  Email,
  Friends,
  Groups,
  Info,
  OutfitsPane,
  PrivacyPane,
  Robux,
  Sessions,
} from "./panesMore";
import { hasCode, msgOf, SignInAgain, ToolCtx } from "./useTool";

const TOOLS: { id: string; key: MsgKey; batch: boolean; Pane: () => ReactNode }[] = [
  { id: "display", key: "tools.displayName", batch: true, Pane: DisplayName },
  { id: "password", key: "tools.password", batch: false, Pane: Password },
  { id: "email", key: "tools.email", batch: false, Pane: Email },
  { id: "privacy", key: "tools.privacy", batch: true, Pane: PrivacyPane },
  { id: "sessions", key: "tools.sessions", batch: false, Pane: Sessions },
  { id: "friends", key: "tools.friends", batch: true, Pane: Friends },
  { id: "blocks", key: "tools.blocks", batch: true, Pane: Blocks },
  { id: "outfits", key: "tools.outfits", batch: false, Pane: OutfitsPane },
  { id: "groups", key: "tools.groups", batch: false, Pane: Groups },
  { id: "robux", key: "tools.robux", batch: false, Pane: Robux },
  { id: "info", key: "tools.info", batch: false, Pane: Info },
];

type Pending = { retry: () => void; cancel: () => void };

/** Rust only needs a userId and a cookie (it returns SESSION_EXPIRED itself): an "unknown"
 * session (no verdict yet, e.g. a network error at import) still works. */
const canSign = (a: AccountView) =>
  a.userId !== null && a.hasCookie && a.session !== "expired" && a.session !== "pending";

const mmss = (ms: number) => {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

/** Account tools: right-side sheet scoped to the selection (one account, or a batch). */
export function ToolsSheet() {
  const open = useUi((u) => u.toolsOpen);
  const sel = useUi((u) => u.sel.ids);
  const { data } = useAccounts();
  const accounts = data?.accounts.filter((a) => sel.includes(a.id)) ?? [];
  const single = accounts.length === 1;
  const [tool, setTool] = useState("display");
  const [pin, setPinState] = useState<Pending | null>(null);
  const pinRef = useRef<Pending | null>(null);
  const singleRef = useRef(single);
  singleRef.current = single;
  const ready = accounts.every(canSign);
  const selKey = sel.join();
  /** Selection whose panes were shown: a session expiring mid-flow keeps the open pane (and its
   * result line) instead of unmounting it; tab switches stay blocked until ready again. */
  const shownFor = useRef("");
  if (ready && accounts.length > 0) shownFor.current = selKey;
  /** No pane at all until every selected account can sign requests. */
  const cur =
    ready || shownFor.current === selKey
      ? (TOOLS.find((x) => x.id === tool && (single || x.batch)) ?? TOOLS[0])
      : undefined;

  /** One pending PIN entry at a time: setting a new one (or null) rejects the previous. */
  const setPin = useCallback((p: Pending | null) => {
    pinRef.current?.cancel();
    pinRef.current = p;
    setPinState(p);
  }, []);

  /** A pending PIN entry belongs to one ready account: losing it (selection change, session
   * gone, sheet unmounted) cancels the entry instead of leaving it hidden. */
  const pinScope = ready && single ? accounts[0].id : "";
  useEffect(() => () => setPin(null), [pinScope, setPin]);
  /** Selection emptied while open (Sheet closes without onOpenChange): reset the flag too. */
  const gone = open && data !== undefined && accounts.length === 0;
  useEffect(() => {
    if (gone) useUi.setState({ toolsOpen: false });
  }, [gone]);

  /** Runs `fn`. PIN_REQUIRED opens the inline prompt (single account) and retries `fn` once
   * unlocked; in a batch it is an error. */
  const inflight = useRef(new Map<string, number>()); // running ops per selection
  const selRef = useRef(selKey);
  selRef.current = selKey;
  const guard = useCallback(
    function g<T>(fn: () => Promise<T>): Promise<T> {
      return fn().catch((e: unknown) => {
        if (hasCode(e, "PIN_REQUIRED")) {
          if (!singleRef.current) throw new Error(t("tools.pin.batch"));
          return new Promise<T>((res, rej) =>
            setPin({
              retry: () => void g(fn).then(res, rej),
              cancel: () => rej(new Error(t("tools.pin.cancelled"))),
            }),
          );
        }
        throw e;
      });
    },
    [setPin],
  );

  /** Counts running ops: a tab switch would unmount the pane mid-op, so it waits (Cancel on a PIN prompt ends it).
   * Counted per selection: an op still running for other accounts doesn't freeze these tabs. */
  const tracked = useCallback(
    <T,>(fn: () => Promise<T>) => {
      const k = selRef.current;
      const m = inflight.current;
      m.set(k, (m.get(k) ?? 0) + 1);
      return guard(fn).finally(() => {
        const n = (m.get(k) ?? 1) - 1;
        if (n) m.set(k, n);
        else {
          m.delete(k);
          setRefused(false);
        }
      });
    },
    [guard],
  );

  /** A close was refused while an op runs: closing would unmount its pane and lose the result. */
  const [refused, setRefused] = useState(false);
  const close = (o: boolean) => {
    if (!o && pin) {
      // the op only waits for a PIN: closing cancels it ("nothing changed"), nothing is lost
      setPin(null);
    } else if (!o && inflight.current.has(selKey)) {
      return setRefused(true);
    }
    useUi.setState({ toolsOpen: o });
  };

  return (
    <Sheet
      open={open && accounts.length > 0}
      onOpenChange={close}
      title={t("tools.title")}
      width={680}
    >
      <ToolCtx.Provider value={{ accounts, guard: tracked }}>
        {accounts.length > 0 && (
          <>
            <div className="mb-4 flex items-center gap-3 border-b border-hairline pb-4">
              <Header accounts={accounts} />
              {single && <PinChip id={accounts[0].id} />}
            </div>
            {refused && (
              <p role="status" className="mb-4 text-13 text-text-2">
                {t("tools.stillWorking")}
              </p>
            )}
            {!ready && (
              <div role="alert" className="mb-5 flex flex-col items-start gap-2">
                <p className="text-13 text-text-1">{t("tools.signFirstBody")}</p>
                {accounts
                  .filter((a) => !canSign(a))
                  .map((a) => (
                    <SignInAgain key={a.id} id={a.id} />
                  ))}
              </div>
            )}
            <Tabs.Root
              key={sel.join()}
              orientation="vertical"
              activationMode="manual"
              value={cur?.id ?? ""}
              onValueChange={(v) => {
                const x = TOOLS.find((y) => y.id === v);
                if (!x || !ready || (!single && !x.batch) || inflight.current.has(selKey)) return;
                setPin(null);
                setTool(v);
              }}
              className="grid grid-cols-[168px_minmax(0,1fr)] gap-6.5"
            >
              <Tabs.List aria-label={t("tools.list")} className="flex flex-col gap-0.5">
                {(single || !ready
                  ? TOOLS
                  : [...TOOLS.filter((x) => x.batch), null, ...TOOLS.filter((x) => !x.batch)]
                ).map((x) => {
                  if (!x)
                    return (
                      <span
                        key="single-only"
                        className="mt-3.5 mb-1 ml-3 text-12 font-medium text-text-3"
                      >
                        {t("tools.singleOnly")}
                      </span>
                    );
                  const off = !ready || (!single && !x.batch);
                  return (
                    <Tabs.Trigger
                      key={x.id}
                      value={x.id}
                      aria-disabled={off || undefined}
                      className="flex min-h-[34px] flex-col items-start justify-center rounded-control px-3 py-1 text-left text-13 font-semibold text-text-2 transition-colors hover:bg-surface-3 hover:text-text-1 aria-disabled:cursor-not-allowed aria-disabled:font-medium aria-disabled:text-text-3 aria-disabled:hover:bg-transparent aria-disabled:hover:text-text-3 data-[state=active]:bg-surface-4 data-[state=active]:text-text-1 data-[state=active]:shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus-visible:shadow-[var(--focus-ring)] data-[state=active]:focus-visible:shadow-[var(--focus-ring)]"
                    >
                      {t(x.key)}
                      {!ready && (
                        <span className="text-12 font-normal text-text-3">
                          {t("tools.signFirst")}
                        </span>
                      )}
                    </Tabs.Trigger>
                  );
                })}
              </Tabs.List>
              {cur && (
                <Tabs.Content value={cur.id} className="min-w-0 outline-none" tabIndex={-1}>
                  <h3 className="mb-4 text-16 font-semibold text-text-1">{t(cur.key)}</h3>
                  {pin && pinScope && (
                    <PinPrompt
                      accountId={pinScope}
                      onDone={() => {
                        // cancelled (or replaced) while unlocking: never re-run that op
                        if (pinRef.current !== pin) return;
                        pinRef.current = null;
                        setPinState(null);
                        pin.retry();
                      }}
                      onCancel={() => setPin(null)}
                    />
                  )}
                  <cur.Pane key={cur.id} />
                </Tabs.Content>
              )}
            </Tabs.Root>
          </>
        )}
      </ToolCtx.Provider>
    </Sheet>
  );
}

function Header({ accounts: a }: { accounts: AccountView[] }) {
  if (a.length === 1)
    return (
      <div className="flex min-w-0 flex-1 items-center gap-3">
        <Avatar a={a[0]} size={40} round />
        <div className="min-w-0">
          <p data-pii className="truncate text-14 font-medium text-text-1">
            {label(a[0])}
          </p>
          <p data-pii className="truncate text-12 text-text-3">
            @{a[0].username}
          </p>
        </div>
      </div>
    );
  return (
    <div className="flex min-w-0 flex-1 items-center gap-3">
      <div className="flex pl-2">
        {a.slice(0, 4).map((x) => (
          <Avatar key={x.id} a={x} size={30} round className="-ml-2 ring-2 ring-glass" />
        ))}
      </div>
      <p className="text-14 font-medium text-text-1 tnum">{t("tools.forN", { n: a.length })}</p>
    </div>
  );
}

/** "PIN unlocked · 4:32" until Roblox re-locks. */
function PinChip({ id }: { id: string }) {
  const q = useQuery({
    queryKey: ["pin", id],
    queryFn: () => call("tool_pin_state", { accountId: id }),
    meta: { silent: true },
  });
  const until = q.data?.unlockedUntil ? Date.parse(q.data.unlockedUntil) : 0;
  const [, tick] = useState(0);
  useEffect(() => {
    if (until <= Date.now()) return;
    const h = setInterval(() => {
      tick((x) => x + 1);
      if (Date.now() >= until) clearInterval(h);
    }, 1000);
    return () => clearInterval(h);
  }, [until]);
  const now = Date.now(); // read each render: late data or a past `until` is never stale
  if (until <= now) return null;
  return (
    <span className="shrink-0 rounded-full bg-surface-3 px-2.5 py-1 text-12 font-medium text-text-1 tnum">
      {t("tools.pin.chip", { time: mmss(until - now) })}
    </span>
  );
}

/** Inline 4-digit PIN prompt. The PIN lives in this component's state only. */
function PinPrompt({
  accountId,
  onDone,
  onCancel,
}: {
  accountId: string;
  onDone: () => void;
  onCancel: () => void;
}) {
  const qc = useQueryClient();
  const [pin, setPin] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const errId = useId();
  const submit = async () => {
    if (!/^\d{4}$/.test(pin)) return setErr(t("tools.pin.bad"));
    setBusy(true);
    setErr(null);
    try {
      qc.setQueryData(["pin", accountId], await call("tool_pin_unlock", { accountId, pin }));
      setPin("");
      onDone();
    } catch (e) {
      setErr(msgOf(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      role="group"
      noValidate
      aria-label={t("tools.pin.title")}
      className="mb-5 flex max-w-[420px] flex-col gap-3 rounded-panel bg-surface-3 p-4"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <p className="text-14 font-medium text-text-1">{t("tools.pin.title")}</p>
      <p className="text-13 text-text-2">{t("tools.pin.body")}</p>
      <label className="flex flex-col gap-1.5">
        <span className="text-13 font-medium text-text-1">{t("tools.pin.label")}</span>
        <input
          className={inp}
          type="password"
          inputMode="numeric"
          maxLength={4}
          autoComplete="off"
          autoFocus
          value={pin}
          aria-invalid={err ? true : undefined}
          aria-describedby={err ? errId : undefined}
          onChange={(e) => setPin(e.target.value.replace(/\D/g, ""))}
        />
      </label>
      {err && (
        <p id={errId} role="alert" className="text-13 text-error">
          {err}
        </p>
      )}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" disabled={busy || !pin}>
          {busy ? t("tools.working") : t("tools.pin.cta")}
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          {t("tools.cancel")}
        </Button>
      </div>
    </form>
  );
}
