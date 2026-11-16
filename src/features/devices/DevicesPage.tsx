import { useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Laptop, Monitor, RotateCw, Server, Smartphone } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/Button";
import { Sheet } from "@/components/ui/Sheet";
import { openExternal } from "@/lib/events";
import { call, errMessage, type RemoteConnect, type RemotePeer, type RemotePeers } from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { inputCls, listRowCls } from "@/features/settings/controls";
import { Step } from "@/features/settings/FriendSetup";
import { RemoteSession } from "./RemoteSession";

const DOWNLOAD = "https://tailscale.com/download";

const OS_ICON = { windows: Monitor, macos: Laptop, linux: Server, ios: Smartphone };

function osIcon(os: RemotePeer["os"]) {
  const Icon = os === "android" ? Smartphone : os === "other" ? Monitor : OS_ICON[os];
  return <Icon size={18} strokeWidth={1.75} aria-hidden />;
}

/** Phones and Linux can't host a session (Linux can only view). */
const cantHost = (p: RemotePeer) => !p.rmi && ["ios", "android", "linux"].includes(p.os);

function statusKey(p: RemotePeer): MsgKey {
  if (!p.online) return "dev.offline";
  if (!p.rmi) return cantHost(p) ? "dev.cantHost" : "dev.noRmi";
  if (!p.paired) return "dev.unpaired";
  return p.busy ? "dev.inUse" : "dev.paired";
}

/** Tailscale missing or signed out: the friend setup's steps, without Funnel. */
function TailscaleGate({ st, os }: { st: RemotePeers; os: string | undefined }) {
  return (
    <div>
      <p className="pb-3 text-13 text-text-2">{t("dev.tsIntro")}</p>
      <ol className="-mx-3">
        <Step
          n={1}
          state={st.installed ? "done" : "current"}
          title={t("fs.step1")}
          desc={t("fs.step1.d")}
        >
          {!st.installed && (
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
          state={st.installed ? "current" : "upcoming"}
          title={t("fs.step2")}
          desc={t(os === "windows" ? "fs.step2.win" : "fs.step2.mac")}
        />
      </ol>
    </div>
  );
}

function PairSheet({
  peer,
  onClose,
  onConnect,
}: {
  peer: RemotePeer | null;
  onClose: () => void;
  onConnect: (p: RemotePeer) => void;
}) {
  const qc = useQueryClient();
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  /** Name the other computer answered with once paired. */
  const [paired, setPaired] = useState<string | null>(null);
  // not while the code is on its way: its answer would land on the next sheet
  const close = () => {
    if (busy) return;
    setCode("");
    setErr(null);
    setPaired(null);
    onClose();
  };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!peer || busy) return;
    setBusy(true);
    setErr(null);
    call("remote_pair", { ip: peer.ip, code: code.trim() })
      .then(
        (name) => {
          void qc.invalidateQueries({ queryKey: ["remotePeers"] });
          setPaired(name);
        },
        (e: unknown) => setErr(errMessage(e)),
      )
      .finally(() => setBusy(false));
  };
  const name = paired ?? peer?.rmi ?? peer?.name ?? "";
  return (
    <Sheet
      centered
      width={420}
      open={!!peer}
      onOpenChange={(o) => !o && close()}
      title={t(paired ? "dev.pairedTitle" : "dev.pairTitle", { name })}
      description={t(paired ? "dev.pairedDesc" : "dev.pairDesc")}
    >
      {paired && peer ? (
        <div className="flex justify-end gap-2 pt-1">
          <Button onClick={close}>{t("dev.done")}</Button>
          <Button
            autoFocus
            variant="primary"
            onClick={() => {
              close();
              onConnect(peer);
            }}
          >
            {t("dev.connect")}
          </Button>
        </div>
      ) : (
        <form onSubmit={submit} className="flex flex-col gap-3">
          <input
            autoFocus
            value={code}
            onChange={(e) => setCode(e.target.value)}
            aria-label={t("dev.code")}
            aria-describedby={err ? "pair-err" : undefined}
            placeholder={t("dev.codePlaceholder")}
            maxLength={12}
            autoComplete="off"
            spellCheck={false}
            className={clsx(inputCls, "font-mono text-16 tracking-[0.2em] uppercase")}
          />
          {err && (
            <p id="pair-err" role="alert" className="text-13 text-error">
              {err}
            </p>
          )}
          <div className="flex justify-end gap-2 pt-1">
            <Button type="button" disabled={busy} onClick={close}>
              {t("set.cancel")}
            </Button>
            <Button type="submit" variant="primary" disabled={busy || code.trim().length < 8}>
              {busy ? t("dev.pairing") : t("dev.pair")}
            </Button>
          </div>
        </form>
      )}
    </Sheet>
  );
}

/** Other computers on the user's tailnet; pair with and control the ones running RMI. */
export function DevicesPage() {
  const qc = useQueryClient();
  const { data, isError, error, isFetching, refetch } = useQuery({
    queryKey: ["remotePeers"],
    queryFn: () => call("remote_peers"),
    refetchInterval: 10_000,
    meta: { silent: true },
  });
  const { data: caps } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  const [pairing, setPairing] = useState<RemotePeer | null>(null);
  const [conn, setConn] = useState<{ c: RemoteConnect; name: string; ip: string } | null>(null);
  /** ip being connected (one at a time: a double click must not open two sessions) */
  const [connecting, setConnecting] = useState<string | null>(null);
  /** ip whose Remove waits for a yes */
  const [asking, setAsking] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const fail = (e: unknown) => setMsg(errMessage(e));
  const done = () => void qc.invalidateQueries({ queryKey: ["remotePeers"] });
  const label = (p: RemotePeer) => p.rmi ?? p.name;

  const connect = (p: RemotePeer) => {
    if (connecting || conn) return;
    setMsg(null);
    setConnecting(p.ip);
    void call("remote_connect", { ip: p.ip })
      .then((c) => setConn({ c, name: label(p), ip: p.ip }), fail)
      .finally(() => setConnecting(null));
  };
  // the other computer no longer knows this one: drop the dead token so Pair comes back
  const ended = (why?: string) => {
    const was = conn;
    setConn(null);
    done(); // its "In use" is stale now
    if (!was || (why !== "auth" && why !== "removed")) return;
    void call("remote_forget", { ip: was.ip }).then(done, fail);
    setMsg(t("dev.removedThere", { name: was.name }));
  };

  const ready = data?.installed && data.running;
  // computers that can never be controlled go last
  const peers = data?.peers
    .map((p, i) => ({ p, i }))
    .sort((a, b) => Number(cantHost(a.p)) - Number(cantHost(b.p)) || a.i - b.i)
    .map((x) => x.p);
  return (
    <div className="mx-auto w-full max-w-[700px] px-10 pt-9 pb-16">
      <div className="flex items-start gap-4">
        <div className="min-w-0 flex-1">
          <h2 className="text-20 font-semibold text-text-1">{t("dev.title")}</h2>
          <p className="mt-0.5 mb-2.5 text-13 text-text-2">{t("dev.desc")}</p>
        </div>
        {ready && (
          <Button sm variant="ghost" disabled={isFetching} onClick={() => void refetch()}>
            <RotateCw size={14} strokeWidth={1.75} aria-hidden />
            {t("dev.refresh")}
          </Button>
        )}
      </div>

      {isError && (
        <div role="alert" className="flex items-center gap-3 py-4 text-13 text-error">
          <p>{errMessage(error)}</p>
          <Button sm onClick={() => void refetch()}>
            {t("error.retry")}
          </Button>
        </div>
      )}
      {!data && !isError && (
        <p role="status" className="py-4 text-13 text-text-3">
          {t("dev.loading")}
        </p>
      )}
      {data && !ready && <TailscaleGate st={data} os={caps?.os} />}
      {ready && data.peers.length === 0 && (
        <div className="py-6">
          <p className="text-14 text-text-1">{t("dev.empty")}</p>
          <p className="mt-1 text-13 text-text-2">{t("dev.empty.d")}</p>
        </div>
      )}
      {ready && peers && peers.length > 0 && (
        <ul className="-mx-3" aria-label={t("dev.title")}>
          {peers.map((p) => (
            <li key={p.ip} className={clsx("flex items-center gap-3.5", listRowCls)}>
              <span className="relative inline-grid size-9 shrink-0 place-items-center rounded-control bg-surface-3 text-text-2">
                {osIcon(p.os)}
                <i
                  aria-hidden
                  className={clsx(
                    "absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2 ring-[var(--bg)]",
                    p.online ? "bg-online" : "bg-text-3",
                  )}
                />
              </span>
              {asking === p.ip ? (
                <div className="flex min-w-0 flex-1 flex-wrap items-center justify-end gap-2">
                  <span role="status" className="mr-auto min-w-0 text-13 text-text-2">
                    {t("dev.removeAsk", { name: label(p) })}
                  </span>
                  <Button
                    sm
                    variant="danger"
                    onClick={() => {
                      setAsking(null);
                      void call("remote_forget", { ip: p.ip }).then(done, fail);
                    }}
                  >
                    {t("dev.remove")}
                  </Button>
                  <Button sm variant="ghost" onClick={() => setAsking(null)}>
                    {t("set.cancel")}
                  </Button>
                </div>
              ) : (
                <>
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-14 text-text-1">{label(p)}</p>
                    <p className="text-13 text-text-3">{t(statusKey(p))}</p>
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    {p.paired ? (
                      <>
                        <Button
                          sm
                          variant="ghost"
                          aria-label={t("dev.removeNamed", { name: label(p) })}
                          onClick={() => setAsking(p.ip)}
                        >
                          {t("dev.remove")}
                        </Button>
                        <Button
                          sm
                          variant="primary"
                          aria-label={t("dev.connectNamed", { name: label(p) })}
                          data-connect={p.ip}
                          disabled={!p.rmi || !!connecting || !!conn}
                          onClick={() => connect(p)}
                        >
                          {connecting === p.ip ? t("dev.connecting") : t("dev.connect")}
                        </Button>
                      </>
                    ) : (
                      p.rmi && (
                        <Button
                          sm
                          variant="primary"
                          aria-label={t("dev.pairTitle", { name: label(p) })}
                          onClick={() => setPairing(p)}
                        >
                          {t("dev.pair")}
                        </Button>
                      )
                    )}
                  </div>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
      {msg && (
        <p role="alert" className="mt-2 text-13 text-error">
          {msg}
        </p>
      )}
      {ready && <p className="mt-5 text-13 text-text-3">{t("dev.hint")}</p>}
      <PairSheet peer={pairing} onClose={() => setPairing(null)} onConnect={connect} />
      {conn && (
        <RemoteSession
          conn={conn.c}
          onClose={ended}
          returnFocus={() => document.querySelector(`[data-connect="${conn.ip}"]`)}
        />
      )}
    </div>
  );
}
