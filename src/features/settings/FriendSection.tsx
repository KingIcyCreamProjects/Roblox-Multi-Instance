import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Copy, Eye, EyeOff } from "lucide-react";
import { useState, type ReactNode } from "react";
import { useSetSettings, useSettings } from "@/app/useSettings";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { useAccountActions, useAccounts } from "@/features/accounts/useAccounts";
import { label } from "@/features/accounts/model";
import { openExternal } from "@/lib/events";
import { call, errMessage } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { listRowCls, SubHeading, Toggle } from "./controls";
import { useDraft } from "./draft";
import { FriendSetup } from "./FriendSetup";

const DOTS = "•".repeat(12);
/** An address friend_ts_enable makes: https *.ts.net on 443, 8443 or 10000. */
const RMI_MADE = /^https:\/\/[^/:@\s]+\.ts\.net(:8443|:10000)?\/?$/i;

/** The message the owner sends the friend. Never holds the password. */
export const inviteText = (link: string, user: string) => t("friend.inviteText", { link, user });

/** The owner login must not reuse the friend's name (names are not case sensitive). */
export const ownerNameClash = (owner: unknown, friend: unknown) => {
  const o = String(owner ?? "")
    .trim()
    .toLowerCase();
  return (
    o !== "" &&
    o ===
      String(friend ?? "")
        .trim()
        .toLowerCase()
  );
};

/** One "what to send" line: label, value, then its buttons. */
function Row({ name, pii, children }: { name: string; pii?: boolean; children: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 py-1">
      <span data-pii={pii || undefined} className="w-24 shrink-0 text-13 text-text-2">
        {name}
      </span>
      {children}
    </div>
  );
}

const iconBtn = { size: 14, strokeWidth: 1.75, "aria-hidden": true } as const;

/** A masked password with Show and Copy. Fetched only when revealed (copy happens in Rust),
 *  never logged. Remount it (key) to forget a shown password after a change. */
function PwLine({
  get,
  copy,
  copied,
  onMsg,
}: {
  get: "friend_password" | "friend_owner_password";
  copy: "friend_password_copy" | "friend_owner_password_copy";
  copied: string;
  onMsg: (text: string, err: boolean) => void;
}) {
  const [pw, setPw] = useState<string | null>(null);
  const [show, setShow] = useState(false);
  const fail = (e: unknown) => onMsg(errMessage(e), true);
  const reveal = () =>
    show
      ? setShow(false)
      : void (pw ? Promise.resolve(pw) : call(get).then((p) => (setPw(p), p))).then(
          () => setShow(true),
          fail,
        );
  return (
    <>
      <code data-pii className="min-w-[24ch] font-mono text-13 text-text-1 select-text">
        {show && pw ? pw : DOTS}
      </code>
      {/* the vault password field's reveal pattern: the tooltip is its name */}
      <IconButton
        className="size-7"
        label={t(show ? "friend.hidePw" : "friend.showPw")}
        aria-pressed={show}
        onClick={reveal}
      >
        {show ? <EyeOff {...iconBtn} /> : <Eye {...iconBtn} />}
      </IconButton>
      {/* Rust copies it: hidden from clipboard history, cleared after 30 s and on lock */}
      <Button
        sm
        aria-label={t(get === "friend_password" ? "friend.copyPw" : "friend.copyOwnerPw")}
        onClick={() => void call(copy).then(() => onMsg(copied, false), fail)}
      >
        <Copy {...iconBtn} />
        {t("friend.copyBtn")}
      </Button>
    </>
  );
}

/** Friend remote extras: status, what to send, owner login password, per-account switches. */
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
  // bumped after a new password: remounts the line so the old one is not shown
  const [rev, setRev] = useState(0);

  const [asking, setAsking] = useState(false);
  const { data: s } = useSettings();

  const ok = (text: string) => () => setMsg({ text, err: false });
  const fail = (e: unknown) => setMsg({ text: errMessage(e), err: true });
  const onMsg = (text: string, err: boolean) => setMsg({ text, err });
  // friend_link reads saved settings: an unsaved public address would copy the old one or fail
  const urlUnsaved = useDraft((d) => "friend.publicUrl" in d.edits);
  const noUrl = !s?.friend.publicUrl.trim();
  const linkWhy = urlUnsaved ? t("friend.linkUnsaved") : noUrl ? t("friend.linkNoUrl") : null;
  const { data: link, error: linkErr } = useQuery({
    queryKey: ["friendLink", s?.friend.publicUrl],
    queryFn: () => call("friend_link"),
    enabled: !linkWhy,
    meta: { silent: true },
  });
  // Rust refuses a saved address it can't use (not https): say why instead of "…" forever
  const why = linkWhy ?? (linkErr && !link ? errMessage(linkErr) : null);
  const user = s?.friend.username ?? "";
  const ownerName = s?.friend.ownerUsername.trim() ?? "";
  // the remote is off: these controls do nothing yet, so the block dims like its catalog rows
  const draftOn = useDraft((d) => d.edits["friend.enabled"]) as boolean | undefined;
  const on = draftOn ?? s?.friend.enabled ?? true;
  const set = useSetSettings();
  const tsMade = RMI_MADE.test(s?.friend.publicUrl.trim() ?? "");
  // Funnel entry off first, then the address: the gate shows the setup again
  const [offBusy, setOffBusy] = useState(false);
  const tsOff = () => {
    if (offBusy) return;
    setOffBusy(true);
    void call("friend_ts_disable")
      .then(
        () =>
          set.mutateAsync({ friend: { publicUrl: "", enabled: false } }).then(
            // drafted friend edits would turn it back on with the next Save
            () =>
              useDraft.setState(({ edits }) => {
                const rest = { ...edits };
                delete rest["friend.enabled"];
                delete rest["friend.publicUrl"];
                return { edits: rest };
              }),
            () => {}, // the shared mutation cache already shows it in the banner
          ),
        fail,
      )
      .finally(() => setOffBusy(false));
  };
  const copyText = (text: string, done: string) =>
    void navigator.clipboard.writeText(text).then(ok(done), fail);

  return (
    <div className={clsx("border-b border-hairline py-3.5", !on && "opacity-60")}>
      <p role="status" className="text-13 text-text-2">
        {st?.running ? t("friend.running") : t("friend.off")}
      </p>
      <SubHeading>{t("friend.send")}</SubHeading>
      <p className="mb-1.5 text-13 text-text-2">{t("friend.send.d")}</p>
      <Row name={t("friend.link")}>
        <code data-pii className="min-w-[30ch] font-mono text-13 break-all text-text-1 select-text">
          {link ?? "…"}
        </code>
        <Button
          sm
          disabled={!!linkWhy}
          aria-label={t("friend.copy")}
          aria-describedby={why ? "friend-link-why" : undefined}
          onClick={() =>
            void call("friend_link")
              .then((s) => navigator.clipboard.writeText(s))
              .then(ok(t("friend.copied")), fail)
          }
        >
          <Copy {...iconBtn} />
          {t("friend.copyBtn")}
        </Button>
        {why && (
          <span id="friend-link-why" className="text-13 text-text-3">
            {why}
          </span>
        )}
      </Row>
      <Row name={t("friend.user")}>
        <code data-pii className="min-w-[30ch] font-mono text-13 text-text-1 select-text">
          {user}
        </code>
        <Button
          sm
          aria-label={t("friend.copyUser")}
          onClick={() => copyText(user, t("friend.userCopied"))}
        >
          <Copy {...iconBtn} />
          {t("friend.copyBtn")}
        </Button>
      </Row>
      <Row name={t("friend.pass")}>
        <PwLine
          key={rev}
          get="friend_password"
          copy="friend_password_copy"
          copied={t("friend.pwCopied")}
          onMsg={onMsg}
        />
      </Row>
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
                void call("friend_new_password").then(() => {
                  setRev((r) => r + 1);
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
            <Button
              sm
              variant="primary"
              disabled={!!linkWhy}
              title={t("friend.inviteD")}
              onClick={() =>
                void call("friend_link")
                  .then((l) => navigator.clipboard.writeText(inviteText(l, user)))
                  .then(ok(t("friend.inviteCopied")), fail)
              }
            >
              <Copy {...iconBtn} />
              {t("friend.invite")}
            </Button>
            <Button sm disabled={!link} onClick={() => link && void openExternal(link).catch(fail)}>
              {t("friend.open")}
            </Button>
            {/* signs the friend out: asks first, like Regenerate token */}
            <Button sm onClick={() => setAsking(true)}>
              {t("friend.newPw")}
            </Button>
            {tsMade && (
              <Button
                sm
                variant="ghost"
                title={t("friend.tsOff.d")}
                disabled={offBusy}
                onClick={tsOff}
              >
                {t("friend.tsOff")}
              </Button>
            )}
          </>
        )}
      </div>
      {ownerName && (
        <>
          <SubHeading>{t("friend.ownerPassword")}</SubHeading>
          <Row name={ownerName} pii>
            <PwLine
              get="friend_owner_password"
              copy="friend_owner_password_copy"
              copied={t("friend.ownerPwCopied")}
              onMsg={onMsg}
            />
          </Row>
        </>
      )}
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
