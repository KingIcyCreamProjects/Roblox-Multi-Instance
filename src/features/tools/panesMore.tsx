import { useQueryClient } from "@tanstack/react-query";
import { ExternalLink, RefreshCw } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/Button";
import { openExternal } from "@/features/accounts/AccountMenu";
import { label } from "@/features/accounts/model";
import { Outfits } from "@/features/discover/Outfits";
import { copyText } from "@/features/discover/shared";
import { call } from "@/lib/ipc";
import { fmtInt, LOCALE, t } from "@/lib/i18n";
import { Field, Form, Go, ids, inp, USER } from "./panes";
import { BatchView, Q, Status, useAction, useBatchAction, useTools, useToolQuery } from "./useTool";

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/;

export function Email() {
  const { accounts } = useTools();
  const id = accounts[0].id;
  const qc = useQueryClient();
  const q = useToolQuery(["email", id], () => call("tool_email_get", { accountId: id }));
  const [email, setEmail] = useState("");
  const [pw, setPw] = useState("");
  const a = useAction();
  const bad = email !== "" && !EMAIL.test(email.trim());
  return (
    <Form
      onSubmit={() =>
        void a.run(
          () => call("tool_email_set", { accountId: id, email: email.trim(), password: pw }),
          t("tools.email.ok"),
          () => {
            setEmail("");
            setPw("");
            void qc.invalidateQueries({ queryKey: ["tool", "email", id] });
          },
        )
      }
    >
      <Q q={q}>
        {(d) => (
          <p className="text-13 text-text-2">
            {t("tools.email.current")}:{" "}
            <span data-pii className="text-text-1">
              {d.masked ?? t("tools.email.none")}
            </span>
            {d.masked && !d.verified && ` (${t("tools.email.unverified")})`}
          </p>
        )}
      </Q>
      <Field label={t("tools.email.new")} error={bad && t("tools.email.bad")}>
        <input
          className={inp}
          data-pii
          type="email"
          autoComplete="off"
          spellCheck={false}
          value={email}
          onChange={(e) => setEmail(e.target.value)}
        />
      </Field>
      <Field label={t("tools.email.password")}>
        <input
          className={inp}
          type="password"
          autoComplete="current-password"
          value={pw}
          onChange={(e) => setPw(e.target.value)}
        />
      </Field>
      <Go disabled={a.busy || !email.trim() || bad || !pw}>{t("tools.email.cta")}</Go>
      <Status s={a.s} />
    </Form>
  );
}

export function PrivacyPane() {
  const { accounts } = useTools();
  const qc = useQueryClient();
  const q = useToolQuery(["privacy", accounts[0].id], () =>
    call("tool_privacy_get", { accountId: accounts[0].id }),
  );
  const [follow, setFollow] = useState<string>();
  const [invite, setInvite] = useState<string>();
  const [none, setNone] = useState(false);
  const b = useBatchAction();
  const batch = accounts.length > 1;
  return (
    <Q q={q}>
      {(d) => {
        // single: pre-filled with the current value, only changes are sent. batch: unchanged by default.
        const fv = follow ?? (batch ? "" : d.follow);
        const iv = invite ?? (batch ? "" : d.privateServerInvite);
        const sel = (
          lbl: string,
          value: string,
          opts: { value: string; label: string }[],
          set: (v: string) => void,
        ) => (
          <Field label={lbl}>
            <select
              className={inp}
              value={value}
              onChange={(e) => {
                set(e.target.value);
                setNone(false);
              }}
            >
              {batch && <option value="">{t("tools.priv.keep")}</option>}
              {opts.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </Field>
        );
        const f = fv && (batch || fv !== d.follow) ? fv : undefined;
        const i = iv && (batch || iv !== d.privateServerInvite) ? iv : undefined;
        return (
          <Form
            onSubmit={() => {
              if (!f && !i) return setNone(true);
              void b
                .run((batchId) =>
                  call("tool_privacy_set", {
                    batchId,
                    accountIds: ids(accounts),
                    ...(f && { follow: f }),
                    ...(i && { privateServerInvite: i }),
                  }),
                )
                .then((r) => {
                  if (!r || r.some((x) => !x.ok)) return;
                  setFollow(undefined);
                  setInvite(undefined);
                  void qc.invalidateQueries({ queryKey: ["tool", "privacy", accounts[0].id] });
                });
            }}
          >
            {sel(t("tools.priv.follow"), fv, d.followOptions, setFollow)}
            {sel(t("tools.priv.invite"), iv, d.privateServerInviteOptions, setInvite)}
            <Go disabled={b.busy || (!f && !i)}>{t("tools.priv.cta")}</Go>
            {none && (
              <p role="alert" className="text-13 text-error">
                {t("tools.priv.nothing")}
              </p>
            )}
            <BatchView b={b} />
          </Form>
        );
      }}
    </Q>
  );
}

export function Sessions() {
  const { accounts } = useTools();
  const [ask, setAsk] = useState(false);
  const a = useAction();
  return (
    <div className="flex max-w-[420px] flex-col gap-4">
      <p className="text-13 text-text-2">{t("tools.sess.body")}</p>
      {ask ? (
        <div role="group" aria-label={t("tools.sess.confirm")} className="flex flex-col gap-3">
          <p className="text-14 text-text-1">{t("tools.sess.confirm")}</p>
          <div className="flex gap-2">
            <Button
              variant="danger"
              disabled={a.busy}
              onClick={() =>
                // the confirm closes on any outcome; the result shows in Status
                void a
                  .run(
                    () => call("tool_signout_others", { accountId: accounts[0].id }),
                    t("tools.sess.ok"),
                  )
                  .then(() => setAsk(false))
              }
            >
              {t("tools.sess.yes")}
            </Button>
            <Button variant="ghost" onClick={() => setAsk(false)}>
              {t("tools.cancel")}
            </Button>
          </div>
        </div>
      ) : (
        <Button className="self-start" onClick={() => setAsk(true)}>
          {t("tools.sess.cta")}
        </Button>
      )}
      <Status s={a.s} />
    </div>
  );
}

function UserField({ v, set }: { v: string; set: (s: string) => void }) {
  const bad = v !== "" && !USER.test(v.trim());
  return (
    <Field label={t("tools.user.label")} error={bad && t("tools.user.bad")}>
      <input
        data-pii
        className={inp}
        value={v}
        onChange={(e) => set(e.target.value)}
        maxLength={30}
        autoComplete="off"
        spellCheck={false}
      />
    </Field>
  );
}

export function Friends() {
  const { accounts } = useTools();
  const [u, setU] = useState("");
  const b = useBatchAction();
  return (
    <Form
      onSubmit={() =>
        void b.run((batchId) =>
          call("tool_friend_request", { batchId, accountIds: ids(accounts), username: u.trim() }),
        )
      }
    >
      <UserField v={u} set={setU} />
      <Go disabled={b.busy || !USER.test(u.trim())}>{t("tools.fr.cta")}</Go>
      <BatchView b={b} />
    </Form>
  );
}

export function Blocks() {
  const { accounts } = useTools();
  const id = accounts[0].id;
  const batch = accounts.length > 1;
  const qc = useQueryClient();
  const [u, setU] = useState("");
  const [ask, setAsk] = useState(false);
  const [done, setDone] = useState<{ count: number; failed: number; truncated: boolean } | null>(
    null,
  );
  const b = useBatchAction();
  const x = useAction();
  // the list only shows for one account: a batch never fetches it
  const list = useToolQuery(
    ["blocked", id],
    () => call("tool_blocked_list", { accountId: id }),
    !batch,
  );
  const refresh = () => void qc.invalidateQueries({ queryKey: ["tool", "blocked", id] });
  const run = (name: string, block: boolean) =>
    b
      .run((batchId) =>
        call("tool_block", { batchId, accountIds: ids(accounts), username: name, block }),
      )
      .then((r) => r && refresh());
  const valid = USER.test(u.trim());
  // block and unblock-all share one busy flag: never both at once on the same account
  const busy = b.busy || x.busy;
  return (
    <div className="flex max-w-[420px] flex-col gap-4">
      <Form onSubmit={() => valid && !busy && void run(u.trim(), true)}>
        <UserField v={u} set={setU} />
        <div className="flex gap-2">
          <Button
            variant="primary"
            disabled={busy || !valid}
            onClick={() => void run(u.trim(), true)}
          >
            {t("tools.bl.block")}
          </Button>
          <Button disabled={busy || !valid} onClick={() => void run(u.trim(), false)}>
            {t("tools.bl.unblock")}
          </Button>
        </div>
        <BatchView b={b} />
      </Form>
      {!batch && (
        <section aria-label={t("tools.bl.list")} className="flex flex-col">
          <Q q={list}>
            {(rows) =>
              rows.length === 0 ? (
                <p className="text-13 text-text-3">{t("tools.bl.none")}</p>
              ) : (
                <>
                  <ul className="flex flex-col">
                    {rows.map((r) => (
                      <li
                        key={r.userId}
                        className="flex h-10 items-center gap-3 border-t border-hairline first:border-t-0"
                      >
                        <span data-pii className="min-w-0 flex-1 truncate text-13 text-text-1">
                          {r.displayName} <span className="text-text-3">@{r.username}</span>
                        </span>
                        <Button
                          variant="ghost"
                          aria-label={t("tools.bl.unblockOne", { name: r.username })}
                          disabled={busy}
                          onClick={() => void run(r.username, false)}
                        >
                          {t("tools.bl.unblock")}
                        </Button>
                      </li>
                    ))}
                  </ul>
                  {ask ? (
                    <div
                      role="group"
                      aria-label={t("tools.bl.all")}
                      className="mt-3 flex items-center gap-2"
                    >
                      <span className="flex-1 text-13 text-text-1">
                        {t("tools.bl.allConfirm", { n: rows.length })}
                      </span>
                      <Button
                        variant="danger"
                        disabled={busy}
                        onClick={() =>
                          void x
                            .run(
                              () => call("tool_unblock_all", { accountId: id }),
                              "",
                              (r) => {
                                setDone(r);
                                refresh();
                              },
                            )
                            .then(() => setAsk(false))
                        }
                      >
                        {t("tools.bl.all")}
                      </Button>
                      <Button variant="ghost" onClick={() => setAsk(false)}>
                        {t("tools.cancel")}
                      </Button>
                    </div>
                  ) : (
                    <Button className="mt-3 self-start" onClick={() => setAsk(true)}>
                      {t("tools.bl.all")}
                    </Button>
                  )}
                  <Status s={x.s} />
                  {done !== null && x.s.k === "ok" && (
                    <>
                      <p role="status" className="mt-2 text-13 text-text-1">
                        {t("tools.bl.allDone", { n: done.count })}
                      </p>
                      {done.failed > 0 && (
                        <p role="alert" className="text-13 text-error">
                          {t("tools.bl.allFailed", { n: done.failed })}
                        </p>
                      )}
                      {done.truncated && (
                        <p className="text-13 text-text-2">{t("tools.bl.allMore")}</p>
                      )}
                    </>
                  )}
                </>
              )
            }
          </Q>
        </section>
      )}
    </div>
  );
}

export function OutfitsPane() {
  const { accounts } = useTools();
  return <Outfits embedded defaultUser={accounts[0].username} />;
}

const groupId = (s: string) =>
  s
    .trim()
    .match(/^(\d+)$|\/(?:groups|communities)\/(\d+)/)
    ?.slice(1)
    .find(Boolean);

export function Groups() {
  const { accounts } = useTools();
  const [g, setG] = useState("");
  const a = useAction();
  const gid = groupId(g);
  const bad = g !== "" && !gid;
  return (
    <Form
      onSubmit={() =>
        void a.run(
          () =>
            call("account_open_site", {
              accountId: accounts[0].id,
              url: `https://www.roblox.com/groups/${gid}`,
            }),
          t("tools.gr.ok"),
        )
      }
    >
      <Field label={t("tools.gr.label")} error={bad && t("tools.gr.bad")}>
        <input
          className={inp}
          value={g}
          onChange={(e) => setG(e.target.value)}
          autoComplete="off"
          spellCheck={false}
        />
      </Field>
      <Go disabled={a.busy || !gid}>
        <ExternalLink size={14} strokeWidth={1.75} />
        <span className="truncate">
          {t("tools.gr.cta")} <span data-pii>{label(accounts[0])}</span>
        </span>
      </Go>
      <Status s={a.s} />
    </Form>
  );
}

export function Robux() {
  const { accounts } = useTools();
  const q = useToolQuery(["robux", accounts[0].id], () =>
    call("tool_robux", { accountId: accounts[0].id }),
  );
  return (
    <div className="flex flex-col items-start gap-3">
      <Q q={q}>
        {(d) => (
          <p className="text-13 text-text-2">
            {t("tools.rbx.label")}
            <span data-pii className="block text-28 font-semibold text-text-1 tnum">
              {fmtInt(d.robux)}
            </span>
          </p>
        )}
      </Q>
      <Button disabled={q.isFetching} onClick={() => void q.refetch()}>
        <RefreshCw size={14} strokeWidth={1.75} />
        {t("tools.rbx.refresh")}
      </Button>
    </div>
  );
}

export function Info() {
  const { accounts } = useTools();
  const id = accounts[0].id;
  const q = useToolQuery(["info", id], () => call("tool_info", { accountId: id }));
  const [saved, setSaved] = useState<string | null>(null);
  const a = useAction();
  return (
    <div className="flex max-w-[420px] flex-col gap-5">
      <Q q={q}>
        {(d) => (
          <>
            <dl className="grid grid-cols-[110px_1fr] gap-x-4 gap-y-2 text-13">
              <dt className="text-text-3">{t("tools.info.age")}</dt>
              <dd className="text-text-1 tnum">{t("tools.info.days", { n: fmtInt(d.ageDays) })}</dd>
              <dt className="text-text-3">{t("tools.info.created")}</dt>
              <dd className="text-text-1 tnum">{new Date(d.created).toLocaleDateString(LOCALE)}</dd>
              <dt className="text-text-3">{t("tools.info.id")}</dt>
              <dd data-pii className="text-text-1 tnum">
                {d.userId}
              </dd>
              <dt className="text-text-3">{t("tools.info.profile")}</dt>
              <dd data-pii className="truncate text-text-1">
                {d.profileUrl}
              </dd>
            </dl>
            <div className="flex gap-2">
              <Button onClick={() => void copyText(d.profileUrl, t("copy.profileLink"))}>
                {t("tools.info.copy")}
              </Button>
              <Button onClick={() => void openExternal(d.profileUrl)}>
                <ExternalLink size={14} strokeWidth={1.75} />
                {t("tools.info.open")}
              </Button>
            </div>
            <div className="flex flex-col items-start gap-3">
              <Button
                disabled={a.busy}
                onClick={() => {
                  setSaved(null);
                  void a.run(
                    () => call("tool_export_summary", { accountId: id }),
                    "",
                    (r) => setSaved(r ? r.path : ""),
                  );
                }}
              >
                {t("tools.info.export")}
              </Button>
              <Status s={a.s} />
              {saved !== null && (
                <p role="status" className="text-13 text-text-1">
                  {saved ? (
                    <>
                      {t("tools.info.exportOk")}{" "}
                      <span data-pii className="break-all text-text-2">
                        {saved}
                      </span>
                    </>
                  ) : (
                    t("tools.info.exportCancelled")
                  )}
                </p>
              )}
            </div>
          </>
        )}
      </Q>
    </div>
  );
}
