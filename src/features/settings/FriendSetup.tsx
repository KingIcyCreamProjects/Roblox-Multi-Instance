import { useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Check } from "lucide-react";
import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/Button";
import { openExternal } from "@/lib/events";
import { call, errMessage, type TsError, type TsStatus } from "@/lib/ipc";
import { t } from "@/lib/i18n";

const DOWNLOAD = "https://tailscale.com/download";
const NONE: TsStatus = { installed: false, running: false, dnsName: null, funnelPort: null };

/** friend_ts_enable rejects with a TsError object; call() keeps it on `cause`. */
const asTsError = (e: unknown): TsError => {
  const c = e instanceof Error ? (e.cause as Partial<TsError> | undefined) : undefined;
  return c?.code
    ? { code: c.code, url: c.url ?? null, message: c.message ?? errMessage(e) }
    : { code: "failed", url: null, message: errMessage(e) };
};

export type State = "done" | "current" | "upcoming";

/** One numbered setup step (also the Devices page's Tailscale gate). */
export function Step({
  n,
  state,
  title,
  desc,
  children,
}: {
  n: number;
  state: State;
  title: string;
  desc: string;
  children?: ReactNode;
}) {
  return (
    <li
      data-state={state}
      aria-current={state === "current" ? "step" : undefined}
      className={clsx(
        "flex items-start gap-3.5 border-b border-hairline px-3 py-3.5",
        state === "current" &&
          "rounded-control bg-surface-2 shadow-[inset_0_0_0_1px_var(--hairline)]",
        state === "upcoming" && "opacity-50",
      )}
    >
      <span
        className={clsx(
          "mt-px inline-grid size-6 shrink-0 place-items-center rounded-full text-13 font-semibold",
          state === "current" && "bg-accent text-accent-ink",
          state === "done" && "bg-surface-3 text-online",
          state === "upcoming" && "bg-surface-3 text-text-2",
        )}
      >
        {state === "done" ? (
          <>
            <Check size={14} strokeWidth={2.25} aria-hidden />
            <span className="sr-only">{t("fs.done")}</span>
          </>
        ) : (
          n
        )}
      </span>
      <div className="min-w-0 flex-1">
        <h3 className="text-14 font-semibold text-text-1">{title}</h3>
        <p className="text-13 text-text-2">{desc}</p>
        {children}
      </div>
    </li>
  );
}

/** Three steps that put the friend server on the user's own Tailscale Funnel. Polls the
 *  Tailscale CLI every 3 s while mounted (react-query pauses it while the window is hidden). */
export function FriendSetup({ onManual }: { onManual: () => void }) {
  const qc = useQueryClient();
  const { data } = useQuery({
    queryKey: ["tsStatus"],
    queryFn: () => call("friend_ts_status"),
    refetchInterval: 3000,
    meta: { silent: true },
  });
  const { data: caps } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  const st = data ?? NONE;
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<TsError | null>(null);

  const enable = () => {
    setBusy(true);
    setErr(null);
    call("friend_ts_enable")
      .then(
        // Rust saved the address: refetching settings swaps this gate for the normal section
        () => qc.invalidateQueries({ queryKey: ["settings"] }),
        (e) => setErr(asTsError(e)),
      )
      .finally(() => setBusy(false));
  };

  const s1: State = st.installed ? "done" : "current";
  const s2: State = !st.installed ? "upcoming" : st.running ? "done" : "current";
  const s3: State = st.running ? "current" : "upcoming";
  const admin = err?.code === "needs_admin";

  return (
    <div>
      <p className="pb-3 text-13 text-text-2">{t("fs.intro")}</p>
      <ol className="-mx-3">
        <Step n={1} state={s1} title={t("fs.step1")} desc={t("fs.step1.d")}>
          {s1 === "current" && (
            <Button
              className="mt-2.5"
              sm
              variant="primary"
              onClick={() => void openExternal(DOWNLOAD)}
            >
              {t("fs.download")}
            </Button>
          )}
        </Step>
        <Step
          n={2}
          state={s2}
          title={t("fs.step2")}
          desc={t(caps?.os === "windows" ? "fs.step2.win" : "fs.step2.mac")}
        />
        <Step n={3} state={s3} title={t("fs.step3")} desc={t("fs.step3.d")}>
          {admin ? (
            <div role="alert" className="mt-2.5 flex flex-col gap-2">
              <p className="text-14 font-semibold text-text-1">{t("fs.admin")}</p>
              <p className="text-13 text-text-2">{t("fs.admin.d")}</p>
              <div className="flex flex-wrap gap-2">
                {err.url && (
                  <Button sm variant="primary" onClick={() => void openExternal(err.url!)}>
                    {t("fs.admin.open")}
                  </Button>
                )}
                <Button sm disabled={busy} onClick={enable}>
                  {t("fs.retry")}
                </Button>
              </div>
            </div>
          ) : (
            <Button
              className="mt-2.5"
              sm
              variant="primary"
              disabled={!st.running || busy}
              onClick={enable}
            >
              {busy ? t("fs.enabling") : t("fs.enable")}
            </Button>
          )}
          {err && !admin && (
            <p role="alert" className="mt-2 text-13 text-error">
              {err.message}
            </p>
          )}
        </Step>
      </ol>
      <button
        type="button"
        onClick={onManual}
        className="mt-3 text-13 text-text-2 underline underline-offset-2 hover:text-text-1"
      >
        {t("fs.own")}
      </button>
    </div>
  );
}
