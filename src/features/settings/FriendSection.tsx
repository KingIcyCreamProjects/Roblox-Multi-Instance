import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Copy, Eye, EyeOff } from "lucide-react";
import { useState, type ReactNode } from "react";
import { useSetSettings, useSettings } from "@/app/useSettings";
import { Button } from "@/components/ui/Button";
import { useAccountActions, useAccounts } from "@/features/accounts/useAccounts";
import { label } from "@/features/accounts/model";
import { call, errMessage } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { listRowCls, SubHeading, Toggle } from "./controls";
import { useDraft } from "./draft";
import { FriendSetup } from "./FriendSetup";

const DOTS = "•".repeat(12);
/** An address friend_ts_enable makes: https *.ts.net on 443, 8443 or 10000. */
const RMI_MADE = /^https:\/\/[^/:@\s]+\.ts\.net(:8443|:10000)?\/?$/i;

/** Friend remote extras: status, link, password, per-account allow switches. The password
 *  is fetched only when the owner reveals it (copy happens in Rust), and never logged. */
export function FriendExtras() {
  const { data: st } = useQuery({
    queryKey: ["friendStatus"],
    queryFn: () => call("friend_status"),
    meta: { silent: true },
  });
  const { data: list } = useAccounts();
  const { update } = useAccountActions();
  const allAllowed = !!list?.accounts.every((a) => a.friendAllowed);
  const [msg, setMsg] = useState<{ text: string; err: boolean } | null>(null);
  const [pw, setPw] = useState<string | null>(null);
  const [show, setShow] = useState(false);

  const [asking, setAsking] = useState(false);
  const { data: s } = useSettings();

  const ok = (text: string) => () => setMsg({ text, err: false });
  const fail = (e: unknown) => setMsg({ text: errMessage(e), err: true });
  // friend_link reads saved settings: an unsaved public address would copy the old one or fail
  const urlUnsaved = useDraft((d) => "friend.publicUrl" in d.edits);
  const noUrl = !s?.friend.publicUrl.trim();
  const linkWhy = urlUnsaved ? t("friend.linkUnsaved") : noUrl ? t("friend.linkNoUrl") : null;
  // the remote is off: these controls do nothing yet, so the block dims like its catalog rows
  const draftOn = useDraft((d) => d.edits["friend.enabled"]) as boolean | undefined;
  const on = draftOn ?? s?.friend.enabled ?? true;
  const getPw = async () => pw ?? call("friend_password").then((p) => (setPw(p), p));
  const set = useSetSettings();
  const tsMade = RMI_MADE.test(s?.friend.publicUrl.trim() ?? "");
  // Funnel entry off first, then the address: the gate shows the setup again
  const tsOff = () =>
    void call("friend_ts_disable")
      .then(() => set.mutateAsync({ friend: { publicUrl: "", enabled: false } }))
      .catch(fail);

  return (
    <div className={clsx("border-b border-hairline py-3.5", !on && "opacity-60")}>
      <p role="status" className="text-13 text-text-2">
        {st?.running ? t("friend.running") : t("friend.off")}
      </p>
      <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-2">
        <Button
          sm
          disabled={!!linkWhy}
          aria-describedby={linkWhy ? "friend-link-why" : undefined}
          onClick={() =>
            void call("friend_link")
              .then((s) => navigator.clipboard.writeText(s))
              .then(ok(t("friend.copied")), fail)
          }
        >
          <Copy size={14} strokeWidth={1.75} aria-hidden />
          {t("friend.copy")}
        </Button>
        {linkWhy && (
          <span id="friend-link-why" className="text-13 text-text-3">
            {linkWhy}
          </span>
        )}
        {tsMade && (
          <Button sm variant="ghost" title={t("friend.tsOff.d")} onClick={tsOff}>
            {t("friend.tsOff")}
          </Button>
        )}
      </div>
      <SubHeading>{t("friend.password")}</SubHeading>
      <div className="flex items-center gap-1">
        <code data-pii className="min-w-[20ch] font-mono text-13 text-text-1 select-text">
          {show && pw ? pw : DOTS}
        </code>
        <Button
          sm
          variant="ghost"
          aria-label={show ? t("friend.hidePw") : t("friend.showPw")}
          aria-pressed={show}
          onClick={() => (show ? setShow(false) : void getPw().then(() => setShow(true), fail))}
        >
          {show ? (
            <EyeOff size={14} strokeWidth={1.75} aria-hidden />
          ) : (
            <Eye size={14} strokeWidth={1.75} aria-hidden />
          )}
        </Button>
      </div>
      <div className="mt-2 flex flex-wrap items-center gap-2">
        {asking ? (
          <>
            <span role="status" className="text-13 text-text-2">
              {t("friend.newPwAsk")}
            </span>
            <Button
              sm
              variant="danger"
              onClick={() => {
                setAsking(false);
                void call("friend_new_password").then((p) => {
                  setPw(p);
                  setMsg({ text: t("friend.pwNew"), err: false });
                }, fail);
              }}
            >
              {t("friend.newPw")}
            </Button>
            <Button sm variant="ghost" onClick={() => setAsking(false)}>
              {t("set.cancel")}
            </Button>
          </>
        ) : (
          <>
            {/* Rust copies it: hidden from clipboard history, cleared after 30 s and on lock */}
            <Button
              sm
              onClick={() => void call("friend_password_copy").then(ok(t("friend.pwCopied")), fail)}
            >
              <Copy size={14} strokeWidth={1.75} aria-hidden />
              {t("friend.copyPw")}
            </Button>
            {/* signs the friend out: asks first, like Regenerate token */}
            <Button sm onClick={() => setAsking(true)}>
              {t("friend.newPw")}
            </Button>
          </>
        )}
      </div>
      {msg && (
        <p
          role={msg.err ? "alert" : "status"}
          className={msg.err ? "mt-2 text-13 text-error" : "mt-2 text-13 text-text-2"}
        >
          {msg.text}
        </p>
      )}
      <div className="flex items-end justify-between gap-3">
        <SubHeading>{t("friend.accounts")}</SubHeading>
        {!!list?.accounts.length && (
          <Button
            sm
            variant="ghost"
            onClick={() =>
              // flips only the switches that differ from the target
              list.accounts
                .filter((a) => a.friendAllowed === allAllowed)
                .forEach((a) => void update(a.id, { friendAllowed: !allAllowed }))
            }
          >
            {t(allAllowed ? "friend.none" : "friend.all")}
          </Button>
        )}
      </div>
      {list?.accounts.length === 0 && (
        <p className="py-2.5 text-13 text-text-3">{t("friend.noAccounts")}</p>
      )}
      {/* the heading says what the switches do; no repeated line under every name */}
      <ul className="-mx-3">
        {list?.accounts.map((a) => (
          <li key={a.id} className={clsx("flex items-center justify-between gap-6", listRowCls)}>
            <div className="min-w-0">
              <p id={`fa-${a.id}-l`} data-pii className="truncate text-14 text-text-1">
                {label(a)}
              </p>
              {a.friendAdded && <p className="text-12 text-text-3">{t("friend.addedBy")}</p>}
            </div>
            <Toggle
              id={`fa-${a.id}`}
              checked={a.friendAllowed}
              onChange={(v) => void update(a.id, { friendAllowed: v })}
            />
          </li>
        ))}
      </ul>
    </div>
  );
}

/** Shows the guided setup until a public address is saved, then the section itself. Installs
 *  that already saved one never see it. "I have my own address" opens the manual fields. */
export function FriendGate({ children }: { children: ReactNode }) {
  const { data: s } = useSettings();
  const [manual, setManual] = useState(false);
  if (s && !s.friend.publicUrl.trim() && !manual)
    return <FriendSetup onManual={() => setManual(true)} />;
  return <>{children}</>;
}
