import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { CircleAlert, CircleCheck } from "lucide-react";
import clsx from "clsx";
import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/Button";
import { Avatar } from "@/features/accounts/AccountRow";
import { label } from "@/features/accounts/model";
import { errText, useAccountActions } from "@/features/accounts/useAccounts";
import {
  codeText,
  hasCode,
  onEvent,
  type AccountView,
  type BatchEvent,
  type BatchResult,
} from "@/lib/ipc";
import { t } from "@/lib/i18n";

export { hasCode };

type Guard = <T>(fn: () => Promise<T>) => Promise<T>;
export const ToolCtx = createContext<{ accounts: AccountView[]; guard: Guard }>({
  accounts: [],
  guard: (fn) => fn(),
});
export const useTools = () => useContext(ToolCtx);

/** Backend text → user text. Strips the "tool_x: " wrapper; machine codes get plain words. */
export function msgOf(e: unknown) {
  if (hasCode(e, "SESSION_EXPIRED")) return t("tools.sessionExpired");
  if (hasCode(e, "PIN_REQUIRED")) return t("tools.pin.batch");
  return errText(e).replace(/^[a-z_]+: /, "") || t("tools.unexpected");
}

type S = { k: "idle" | "busy" | "ok" | "err"; msg: string; expired?: boolean };
const idle: S = { k: "idle", msg: "" };

/** Pending/ok/error state for one tool action. Secrets never enter it, only messages. */
export function useAction() {
  const { guard } = useTools();
  const [s, setS] = useState<S>(idle);
  const run = async <T,>(
    fn: () => Promise<T>,
    ok = "",
    onOk?: (r: T) => void,
  ): Promise<T | undefined> => {
    setS({ k: "busy", msg: "" });
    try {
      const r = await guard(fn);
      onOk?.(r);
      setS({ k: "ok", msg: ok });
      return r;
    } catch (e) {
      setS({ k: "err", msg: msgOf(e), expired: hasCode(e, "SESSION_EXPIRED") });
      return undefined;
    }
  };
  return { s, busy: s.k === "busy", run, reset: () => setS(idle) };
}

/** Starts the interactive sign-in for one account. */
export function SignInAgain({ id }: { id: string }) {
  const { accounts } = useTools();
  const { signIn } = useAccountActions();
  const a = accounts.find((x) => x.id === id);
  if (!a) return null;
  return <Button onClick={() => void signIn(a.id, a.username)}>{t("tools.signInAgain")}</Button>;
}

/** Inline pending / success / error line under a form. */
export function Status({ s }: { s: S }) {
  const { accounts } = useTools();
  if (s.k === "idle") return null;
  if (s.k === "busy")
    return (
      <p role="status" className="text-13 text-text-2">
        {t("tools.working")}
      </p>
    );
  if (s.k === "ok")
    return s.msg ? (
      <p role="status" className="flex items-center gap-2 text-13 text-text-1">
        <CircleCheck size={15} strokeWidth={1.75} className="shrink-0 text-online" />
        {s.msg}
      </p>
    ) : null;
  return (
    <div role="alert" className="flex flex-col items-start gap-2">
      <p className="flex items-start gap-2 text-13 text-error">
        <CircleAlert size={15} strokeWidth={1.75} className="mt-0.5 shrink-0" />
        {s.msg}
      </p>
      {s.expired && accounts[0] && <SignInAgain id={accounts[0].id} />}
    </div>
  );
}

/** useAction for batch commands: live rows from `tools://batch` (own batchId, in flight only). */
export function useBatchAction() {
  const a = useAction();
  const [rows, setRows] = useState<BatchResult[]>([]);
  const [prog, setProg] = useState<{ done: number; total: number } | null>(null);
  const active = useRef<string | null>(null);
  useEffect(
    () =>
      onEvent<BatchEvent>("tools://batch", (e) => {
        if (e.batchId !== active.current) return;
        setRows((r) => [...r.filter((x) => x.accountId !== e.result.accountId), e.result]);
        setProg({ done: e.done, total: e.total });
      }),
    [],
  );
  const run = async (fn: (batchId: string) => Promise<BatchResult[]>) => {
    const id = crypto.randomUUID();
    active.current = id;
    setRows([]);
    setProg(null);
    try {
      const r = await a.run(async () => {
        const r = await fn(id);
        // one account: a PIN row means the same as a thrown PIN_REQUIRED (prompt, then retry)
        if (r.length === 1 && hasCode(r[0].message, "PIN_REQUIRED")) {
          setRows([]);
          throw r[0].message;
        }
        return r;
      });
      if (r) setRows(r);
      return r;
    } finally {
      active.current = null;
      setProg(null);
    }
  };
  return { ...a, run, rows, prog };
}

export function BatchView({ b }: { b: ReturnType<typeof useBatchAction> }) {
  const { accounts } = useTools();
  return (
    <>
      <Status s={b.s} />
      {b.busy && b.prog && (
        <p role="status" className="text-13 text-text-2 tnum">
          {t("tools.progress", b.prog)}
        </p>
      )}
      {b.rows.length > 0 && (
        <div className="mt-1 flex flex-col">
          <p className="mb-1.5 flex items-center justify-between text-12 font-semibold text-text-2 tnum">
            {t("tools.results")}
            <span className="font-normal text-text-3">
              {t("tools.progress", { done: b.rows.length, total: accounts.length })}
            </span>
          </p>
          <ul aria-label={t("tools.results")} className="flex flex-col">
            {b.rows.map((r) => {
              const a = accounts.find((x) => x.id === r.accountId);
              const Dot = r.ok ? CircleCheck : CircleAlert;
              return (
                <li
                  key={r.accountId}
                  className="flex min-h-10 items-start gap-2.5 border-t border-hairline py-2 text-13"
                >
                  {a && <Avatar a={a} size={22} round />}
                  <p data-pii className="min-w-0 flex-1 truncate font-medium text-text-1">
                    {a ? label(a) : r.accountId}
                  </p>
                  <div className="flex max-w-[60%] flex-col items-end gap-1.5">
                    <p
                      className={clsx(
                        "flex items-start gap-1.5 text-right text-12",
                        r.ok ? "text-text-2" : "text-error",
                      )}
                    >
                      <Dot
                        size={13}
                        strokeWidth={1.75}
                        aria-label={t(r.ok ? "tools.ok" : "tools.failed")}
                        className={clsx("mt-px shrink-0", r.ok ? "text-online" : "text-error")}
                      />
                      {r.message
                        ? r.ok
                          ? codeText(r.message)
                          : msgOf(r.message)
                        : r.ok
                          ? t("tools.done")
                          : t("tools.unexpected")}
                    </p>
                    {!r.ok && hasCode(r.message, "SESSION_EXPIRED") && (
                      <SignInAgain id={r.accountId} />
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </>
  );
}

/** Query body with inline pending and error-with-retry states. */
export function Q<T>({ q, children }: { q: UseQueryResult<T>; children: (d: T) => ReactNode }) {
  const { accounts } = useTools();
  if (q.isPending)
    return (
      <p role="status" className="text-13 text-text-2">
        {t("tools.working")}
      </p>
    );
  if (q.isError)
    return (
      <div role="alert" className="flex flex-col items-start gap-2">
        <p className="text-13 text-error">{msgOf(q.error)}</p>
        {hasCode(q.error, "SESSION_EXPIRED") && accounts[0] ? (
          <SignInAgain id={accounts[0].id} />
        ) : (
          <Button onClick={() => void q.refetch()}>{t("error.retry")}</Button>
        )}
      </div>
    );
  return <>{children(q.data)}</>;
}

/** Tool data queries: no shell banner (errors show inline), always fresh on open. */
export function useToolQuery<T>(key: unknown[], fn: () => Promise<T>, enabled = true) {
  return useQuery({
    queryKey: ["tool", ...key],
    queryFn: fn,
    enabled,
    meta: { silent: true },
    staleTime: 0,
  });
}
