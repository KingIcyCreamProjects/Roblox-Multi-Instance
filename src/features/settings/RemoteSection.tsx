import { useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { useEffect, useState } from "react";
import { useSettings } from "@/app/useSettings";
import { Button } from "@/components/ui/Button";
import { call, errMessage, type RemoteStatus } from "@/lib/ipc";
import { LOCALE, t } from "@/lib/i18n";
import { listRowCls, SubHeading } from "./controls";
import { useDraft } from "./draft";

const when = (iso: string) =>
  new Date(iso).toLocaleString(LOCALE, { dateStyle: "medium", timeStyle: "short" });

/** Whole units only (floor): 3599 s is "59 min", never "60 min" or "1 h 60 min". */
export const dur = (sec: number) => {
  const m = Math.floor(sec / 60);
  return sec < 60
    ? t("rs.sec", { n: sec })
    : m < 60
      ? t("rs.min", { n: m })
      : t("rs.hours", { h: Math.floor(m / 60), m: m % 60 });
};

const unit = (unit: string) =>
  new Intl.NumberFormat(LOCALE, { style: "unit", unit, maximumFractionDigits: 1 });
const [kb, mb, gb] = ["kilobyte", "megabyte", "gigabyte"].map(unit);
/** Decimal units: 1 500 000 bytes is "1.5 MB", under 1 kB plain bytes ("48 B"). */
export const size = (b: number) =>
  b >= 1e9
    ? gb.format(b / 1e9)
    : b >= 1e6
      ? mb.format(b / 1e6)
      : b >= 1e3
        ? kb.format(b / 1e3)
        : `${b} B`;

type Log = RemoteStatus["log"][number];

/** "2 files sent, 1 file received" for a session's transfers ("" when none). */
export const filesLine = (files: Log["files"]) => {
  const n = (d: "sent" | "received") => files.filter((f) => f.dir === d).length;
  const [s, r] = [n("sent"), n("received")];
  return [
    s && (s === 1 ? t("rs.fileSent") : t("rs.filesSent", { n: s })),
    r && (r === 1 ? t("rs.fileReceived") : t("rs.filesReceived", { n: r })),
  ]
    .filter(Boolean)
    .join(", ");
};

/** Remote access extras: status, pairing code, paired devices, recent sessions. */
export function RemoteExtras() {
  const qc = useQueryClient();
  const {
    data: st,
    isError,
    error,
    refetch,
  } = useQuery({
    queryKey: ["remoteStatus"],
    queryFn: () => call("remote_status"),
    meta: { silent: true },
  });
  const { data: s } = useSettings();
  const { data: caps } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  const draftOn = useDraft((d) => d.edits["remote.enabled"]) as boolean | undefined;
  const on = draftOn ?? s?.remote.enabled ?? false;
  const saved = !!s?.remote.enabled;
  const [code, setCode] = useState<{ code: string; until: number } | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [err, setErr] = useState<string | null>(null);
  const refresh = () => void qc.invalidateQueries({ queryKey: ["remoteStatus"] });

  // the code counts down while shown and disappears when it expires
  useEffect(() => {
    if (!code) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [code]);
  const left = code ? Math.max(0, Math.ceil((code.until - now) / 1000)) : 0;
  useEffect(() => {
    if (code && left === 0) setCode(null);
  }, [code, left]);
  // switched off: a shown code is dead
  useEffect(() => {
    if (!saved) setCode(null);
  }, [saved]);
  // used by a computer, or burned by wrong tries: gone too
  const codeLive = st?.codeLive;
  useEffect(() => {
    if (codeLive === false) setCode(null);
  }, [codeLive]);

  const showCode = () =>
    void call("remote_pair_code").then(
      (c) => {
        setErr(null);
        setNow(Date.now());
        setCode({ code: c.code, until: Date.now() + c.expiresInSec * 1000 });
        refresh(); // codeLive turns true, so its later false hides the code
      },
      (e) => setErr(errMessage(e)),
    );

  const statusText = !saved
    ? t("rs.off")
    : !st
      ? t("rs.loading")
      : st.listening
        ? t("rs.listening", { name: st.name, addr: st.listening })
        : t("rs.waiting");

  return (
    <div className={clsx("border-b border-hairline py-3.5", !on && "opacity-60")}>
      {isError ? (
        <div role="alert" className="flex items-center gap-3 text-13 text-error">
          <p>{errMessage(error)}</p>
          <Button sm onClick={() => void refetch()}>
            {t("error.retry")}
          </Button>
        </div>
      ) : (
        <p role="status" className="text-13 text-text-2">
          {statusText}
        </p>
      )}
      {saved && caps?.os === "windows" && (
        <p className="mt-1 text-13 text-text-3">{t("rs.firewall")}</p>
      )}
      {st?.session && (
        <div
          role="alert"
          className="mt-2 flex items-center gap-3 rounded-control bg-error/10 px-3 py-2.5"
        >
          <p className="flex-1 text-13 text-text-1">
            {t(st.session.control ? "rs.liveControl" : "rs.liveView", { peer: st.session.peer })}
          </p>
          <Button sm variant="danger" onClick={() => void call("remote_stop").then(refresh)}>
            {t("rs.stop")}
          </Button>
        </div>
      )}

      <SubHeading>{t("rs.pairing")}</SubHeading>
      <p className="text-13 text-text-2">{t("rs.pairing.d")}</p>
      <div className="mt-2 flex flex-wrap items-center gap-3">
        <Button sm disabled={!saved} onClick={showCode}>
          {code ? t("rs.newCode") : t("rs.showCode")}
        </Button>
        {!saved && <span className="text-13 text-text-3">{t("rs.saveFirst")}</span>}
        {code && (
          <>
            <code
              aria-label={t("rs.code")}
              className="rounded-control bg-surface-2 px-3 py-1 font-mono text-20 font-semibold tracking-[0.2em] text-text-1 shadow-[inset_0_0_0_1px_var(--hairline)] select-text"
            >
              {code.code.slice(0, 4)} {code.code.slice(4)}
            </code>
            <span role="timer" className="text-13 text-text-3 tabular-nums">
              {t("rs.expires", { m: Math.floor(left / 60), s: String(left % 60).padStart(2, "0") })}
            </span>
          </>
        )}
      </div>
      {err && (
        <p role="alert" className="mt-2 text-13 text-error">
          {err}
        </p>
      )}

      <SubHeading>{t("rs.paired")}</SubHeading>
      {st?.paired.length === 0 && <p className="py-2.5 text-13 text-text-3">{t("rs.noPaired")}</p>}
      <ul className="-mx-3">
        {st?.paired.map((p) => (
          <li key={p.id} className={clsx("flex items-center justify-between gap-6", listRowCls)}>
            <div className="min-w-0">
              <p className="truncate text-14 text-text-1">{p.name}</p>
              <p className="text-12 text-text-3">{t("rs.pairedAt", { when: when(p.pairedAt) })}</p>
            </div>
            <Button
              sm
              variant="ghost"
              aria-label={t("rs.removeNamed", { name: p.name })}
              onClick={() => void call("remote_unpair", { id: p.id }).then(refresh)}
            >
              {t("rs.remove")}
            </Button>
          </li>
        ))}
      </ul>

      <SubHeading>{t("rs.log")}</SubHeading>
      {st?.log.length === 0 && <p className="py-2.5 text-13 text-text-3">{t("rs.noLog")}</p>}
      <ul className="-mx-3">
        {st?.log.map((l, i) => (
          <li
            // same start second and peer can repeat; the index makes it unique
            key={`${l.startedAt}|${l.peer}|${i}`}
            className={clsx("flex items-center justify-between gap-6", listRowCls)}
          >
            <div className="min-w-0">
              <p className="truncate text-14 text-text-1">{l.peer}</p>
              <p className="text-12 text-text-3">{when(l.startedAt)}</p>
              {l.files.length > 0 && (
                <details className="text-12 text-text-3">
                  <summary className="cursor-pointer hover:text-text-2">
                    {filesLine(l.files)}
                  </summary>
                  <ul className="mt-1 space-y-0.5">
                    {l.files.map((f, j) => (
                      <li key={`${f.name}|${j}`} className="flex min-w-0 gap-1">
                        {/* a long name truncates; size and direction stay visible */}
                        <span className="truncate">{f.name}</span>
                        <span className="shrink-0">
                          {t("rs.fileLine", {
                            size: size(f.size),
                            dir: t(f.dir === "sent" ? "rs.sent" : "rs.received"),
                          })}
                        </span>
                      </li>
                    ))}
                  </ul>
                </details>
              )}
            </div>
            <p className="shrink-0 text-13 text-text-2">
              {t("rs.logLine", {
                mode: t(l.control ? "rs.control" : "rs.viewOnly"),
                dur: dur(l.durationSec),
              })}
            </p>
          </li>
        ))}
      </ul>
    </div>
  );
}
