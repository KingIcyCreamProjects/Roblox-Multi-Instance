import { useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { useState, type FormEvent, type ReactNode } from "react";
import { toast } from "@/components/Toasts";
import { Button } from "@/components/ui/Button";
import { call, pickOpenPath, pickSavePath } from "@/lib/ipc";
import { reportError } from "@/lib/report";
import { LOCALE, t, type MsgKey } from "@/lib/i18n";
import { PasswordField } from "@/features/vault/PasswordField";
import { inputCls, listRowCls, SubHeading } from "./controls";
import { useVaultStatus, vaultError } from "@/features/vault/useVault";

const PHRASE = "export plaintext";

function Block({
  title,
  desc,
  wide,
  children,
}: {
  title: string;
  desc: string;
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <section className="border-b border-hairline pb-3.5">
      <SubHeading>{title}</SubHeading>
      <p className="mb-3 text-13 text-text-2">{desc}</p>
      <div className={clsx("flex flex-col gap-3", !wide && "max-w-sm")}>{children}</div>
    </section>
  );
}

/** Result line: errors announce as alerts, success as status. */
function Msg({ err, ok }: { err: string | null; ok: string | null }) {
  if (err)
    return (
      <p role="alert" className="text-13 text-error">
        {err}
      </p>
    );
  return ok ? (
    <p role="status" className="text-13 text-text-2">
      {ok}
    </p>
  ) : null;
}

/** Runs `fn` with pending/error/success state; secrets stay in the caller's local state. */
function useAction() {
  const [pending, setPending] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [ok, setOk] = useState<string | null>(null);
  const run = async (fn: () => Promise<boolean | void>, done: MsgKey) => {
    setPending(true);
    setErr(null);
    setOk(null);
    try {
      if ((await fn()) !== false) setOk(t(done));
    } catch (e) {
      setErr(vaultError(e));
    } finally {
      setPending(false);
    }
  };
  return { pending, err, ok, setErr, run };
}

/** current and/or new password form; `fields` picks which inputs show. */
function PasswordForm({
  title,
  desc,
  current,
  next,
  submit,
  done,
}: {
  title: string;
  desc: string;
  current: boolean;
  next: boolean;
  submit: string;
  done: MsgKey;
}) {
  const qc = useQueryClient();
  const [cur, setCur] = useState("");
  const [pw, setPw] = useState("");
  const [cf, setCf] = useState("");
  const [touched, setTouched] = useState(false);
  const a = useAction();
  const shortErr = touched && next && [...pw].length < 8 ? t("vault.err.short") : null;
  const matchErr = touched && next && !shortErr && pw !== cf ? t("vault.err.mismatch") : null;

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (a.pending || (current && !cur) || (next && ([...pw].length < 8 || pw !== cf))) return;
    void a.run(async () => {
      await call("vault_change_password", {
        current: current ? cur : null,
        new: next ? pw : null,
      });
      setCur("");
      setPw("");
      setCf("");
      setTouched(false);
      // A mode switch swaps this form out on the refetch, so its inline status would never
      // show: toast it instead.
      const modeSwitch = current !== next;
      if (modeSwitch) toast(t(done));
      await qc.invalidateQueries({ queryKey: ["vault"] });
      return !modeSwitch;
    }, done);
  };

  return (
    <form onSubmit={onSubmit} noValidate>
      <Block title={title} desc={desc}>
        {current && (
          <PasswordField
            showLabel
            label={t("vs.current")}
            autoComplete="current-password"
            value={cur}
            error={touched && !cur ? t("vault.err.empty") : null}
            onChange={(e) => setCur(e.target.value)}
          />
        )}
        {next && (
          <>
            <PasswordField
              showLabel
              label={t("vs.new")}
              autoComplete="new-password"
              value={pw}
              error={shortErr}
              onChange={(e) => setPw(e.target.value)}
            />
            <PasswordField
              showLabel
              label={t("vs.confirmNew")}
              autoComplete="new-password"
              value={cf}
              error={matchErr}
              onChange={(e) => setCf(e.target.value)}
            />
          </>
        )}
        <Msg err={a.err} ok={a.ok} />
        <Button type="submit" variant="primary" disabled={a.pending} className="self-start">
          {a.pending ? t("vs.working") : submit}
        </Button>
      </Block>
    </form>
  );
}

function ago(iso: string) {
  const s = (Date.now() - new Date(iso).getTime()) / 1000;
  // past a week a date reads better than "311 days ago"
  if (s >= 7 * 86400) return new Date(iso).toLocaleDateString(LOCALE);
  const day = Math.floor(s / 86400);
  if (day >= 1) return day === 1 ? t("vs.ago.day1") : t("vs.ago.day", { n: day });
  const hour = Math.floor(s / 3600);
  if (hour >= 1) return hour === 1 ? t("vs.ago.hour1") : t("vs.ago.hour", { n: hour });
  if (s >= 60) return t("vs.ago.min", { n: Math.floor(s / 60) });
  return t("vs.ago.now");
}
const size = (b: number) =>
  b < 1024 * 1024
    ? t("vs.kb", { n: Math.max(1, Math.round(b / 1024)) })
    : t("vs.mb", { n: (b / 1024 / 1024).toFixed(1) });

function Backups({ pw }: { pw: boolean }) {
  const qc = useQueryClient();
  const { data, isError, isPending } = useQuery({
    queryKey: ["vault", "backups"],
    queryFn: () => call("vault_backups"),
    staleTime: 0, // account saves create backups without invalidating this
  });
  const [asking, setAsking] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [ok, setOk] = useState(false);
  // Payload swap under the current password; the vault stays open. Failures go to the
  // shell banner so they survive this section unmounting (e.g. the vault locked meanwhile).
  const restore = async (name: string) => {
    setPending(true);
    setOk(false);
    try {
      await call("vault_restore", { name, password: null });
      setAsking(null);
      setOk(true);
    } catch (e) {
      reportError(e);
    } finally {
      setPending(false);
      await qc.invalidateQueries({ queryKey: ["vault"] });
      await qc.invalidateQueries({ queryKey: ["accounts"] });
    }
  };

  return (
    <Block title={t("vs.backups")} desc={t("vs.backups.d")} wide>
      {isPending && <p className="text-13 text-text-3">{t("vs.backups.loading")}</p>}
      {isError && (
        <p role="alert" className="text-13 text-error">
          {t("vs.backups.err")}
        </p>
      )}
      {data?.length === 0 && <p className="text-13 text-text-3">{t("vs.backups.none")}</p>}
      <ul className="-mx-3 flex flex-col">
        {data?.map((b) => (
          <li key={b.name} className={clsx("flex flex-wrap items-center gap-3", listRowCls)}>
            <span className="text-14 text-text-1">{ago(b.createdAt)}</span>
            <span className="text-13 text-text-2 tabular-nums">{size(b.size)}</span>
            {asking === b.name ? (
              <div className="flex w-full flex-wrap items-center gap-2 pt-1">
                <p className="text-13 text-text-2">
                  {t(pw ? "vs.restore.ask" : "vs.restore.askOs")}
                </p>
                <Button sm variant="danger" disabled={pending} onClick={() => void restore(b.name)}>
                  {pending ? t("vs.working") : t("vs.restore.yes")}
                </Button>
                <Button sm variant="ghost" disabled={pending} onClick={() => setAsking(null)}>
                  {t("vs.restore.no")}
                </Button>
              </div>
            ) : (
              <Button
                sm
                variant="ghost"
                className="ml-auto"
                aria-label={`${t("vs.restore")} ${ago(b.createdAt)}`}
                onClick={() => {
                  setAsking(b.name);
                  setOk(false);
                }}
              >
                {t("vs.restore")}
              </Button>
            )}
          </li>
        ))}
      </ul>
      <Msg err={null} ok={ok ? t("vs.restored") : null} />
    </Block>
  );
}

function ExportEncrypted({ needPassword }: { needPassword: boolean }) {
  const [cur, setCur] = useState("");
  const [pw, setPw] = useState("");
  const [cf, setCf] = useState("");
  const [touched, setTouched] = useState(false);
  const a = useAction();
  const shortErr = touched && [...pw].length < 8 ? t("vault.err.short") : null;
  const matchErr = touched && !shortErr && pw !== cf ? t("vault.err.mismatch") : null;
  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (a.pending || [...pw].length < 8 || pw !== cf || (needPassword && !cur)) return;
    void a.run(async () => {
      const path = await pickSavePath("rmi-vault.rmiv", "rmiv", t("vs.rmivFiles"));
      if (!path) return false;
      await call("vault_export_encrypted", {
        path,
        password: pw,
        current: needPassword ? cur : null,
      });
      setCur("");
      setPw("");
      setCf("");
      setTouched(false);
    }, "vs.exported");
  };
  return (
    <form onSubmit={onSubmit} noValidate>
      <Block title={t("vs.exportEnc")} desc={t("vs.exportEnc.d")}>
        {needPassword && (
          <PasswordField
            showLabel
            label={t("vs.current")}
            autoComplete="current-password"
            value={cur}
            error={touched && !cur ? t("vault.err.empty") : null}
            onChange={(e) => setCur(e.target.value)}
          />
        )}
        <PasswordField
          showLabel
          label={t("vs.exportPassword")}
          autoComplete="new-password"
          value={pw}
          error={shortErr}
          onChange={(e) => setPw(e.target.value)}
        />
        <PasswordField
          showLabel
          label={t("vault.confirm")}
          autoComplete="new-password"
          value={cf}
          error={matchErr}
          onChange={(e) => setCf(e.target.value)}
        />
        <Msg err={a.err} ok={a.ok} />
        <Button type="submit" disabled={a.pending} className="self-start">
          {a.pending ? t("vs.working") : t("vs.exportEncBtn")}
        </Button>
      </Block>
    </form>
  );
}

/** Loads an "Export encrypted" file from another computer; the backend backs up first. */
function ImportEncrypted() {
  const qc = useQueryClient();
  const [pw, setPw] = useState("");
  /** it replaces every account: asks first, like Restore */
  const [asking, setAsking] = useState(false);
  const a = useAction();
  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (a.pending || !pw) return;
    setAsking(true);
  };
  const run = () => {
    setAsking(false);
    void a.run(async () => {
      const path = await pickOpenPath("rmiv", t("vs.rmivFiles"));
      if (!path) return false;
      try {
        await call("vault_import_file", { path, password: pw });
      } finally {
        // the backup Rust made before the import shows in Backups
        void qc.invalidateQueries({ queryKey: ["vault"] });
      }
      setPw("");
    }, "vs.imported");
  };
  return (
    <form onSubmit={onSubmit} noValidate>
      <Block title={t("vs.import")} desc={t("vs.import.d")}>
        <PasswordField
          showLabel
          label={t("vs.exportPassword")}
          autoComplete="off"
          value={pw}
          onChange={(e) => setPw(e.target.value)}
        />
        <Msg err={a.err} ok={a.ok} />
        {asking ? (
          <div className="flex flex-wrap items-center gap-2">
            <p role="status" className="text-13 text-text-2">
              {t("vs.import.ask")}
            </p>
            <Button sm variant="danger" disabled={!pw} onClick={run}>
              {t("vs.importBtn")}
            </Button>
            <Button autoFocus sm variant="ghost" onClick={() => setAsking(false)}>
              {t("vs.restore.no")}
            </Button>
          </div>
        ) : (
          <Button type="submit" disabled={!pw || a.pending} className="self-start">
            {a.pending ? t("vs.working") : t("vs.importBtn")}
          </Button>
        )}
      </Block>
    </form>
  );
}

function ExportPlain({ needPassword }: { needPassword: boolean }) {
  const [phrase, setPhrase] = useState("");
  const [pw, setPw] = useState("");
  const a = useAction();
  const ready = phrase === PHRASE && (!needPassword || pw !== "");
  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (!ready || a.pending) return;
    void a.run(async () => {
      const path = await pickSavePath("rmi-vault-plain.json", "json", t("settings.jsonFiles"));
      if (!path) return false;
      await call("vault_export_plain", {
        path,
        confirm: phrase,
        password: needPassword ? pw : null,
      });
      setPhrase("");
      setPw("");
    }, "vs.exported");
  };
  return (
    <form onSubmit={onSubmit} noValidate>
      <Block title={t("vs.exportPlain")} desc={t("vs.exportPlain.d")}>
        <p className="text-13 text-error">{t("vs.exportPlain.warn")}</p>
        <div className="flex flex-col gap-1.5">
          <label htmlFor="plain-phrase" className="text-13 font-medium text-text-1">
            {t("vs.typePhrase")}
          </label>
          <input
            id="plain-phrase"
            value={phrase}
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => setPhrase(e.target.value)}
            className={clsx(inputCls, "w-full")}
          />
        </div>
        {needPassword && (
          <PasswordField
            showLabel
            label={t("vs.current")}
            autoComplete="current-password"
            value={pw}
            onChange={(e) => setPw(e.target.value)}
          />
        )}
        <Msg err={a.err} ok={a.ok} />
        <Button
          type="submit"
          variant="danger"
          disabled={!ready || a.pending}
          className="self-start"
        >
          {a.pending ? t("vs.working") : t("vs.exportPlainBtn")}
        </Button>
      </Block>
    </form>
  );
}

export function VaultExtras() {
  const { data: v } = useVaultStatus();
  if (!v) return null;
  if (!v.mode) return <p className="py-3.5 text-13 text-text-2">{t("vs.noMode")}</p>;
  const pw = v.mode === "password";
  return (
    <div>
      <p className="py-3.5 text-13 text-text-2">{t(pw ? "vs.mode.password" : "vs.mode.os")}</p>
      {pw ? (
        <>
          <PasswordForm
            key="change"
            title={t("vs.change")}
            desc={t("vs.change.d")}
            current
            next
            submit={t("vs.changeBtn")}
            done="vs.changed"
          />
          <PasswordForm
            key="toOs"
            title={t("vs.toOs")}
            desc={t("vs.toOs.d")}
            current
            next={false}
            submit={t("vs.toOsBtn")}
            done="vs.toOsDone"
          />
        </>
      ) : (
        <PasswordForm
          key="toPw"
          title={t("vs.toPw")}
          desc={t("vs.toPw.d")}
          current={false}
          next
          submit={t("vs.toPwBtn")}
          done="vs.toPwDone"
        />
      )}
      <Backups pw={pw} />
      <ExportEncrypted needPassword={pw} />
      <ImportEncrypted />
      <ExportPlain needPassword={pw} />
    </div>
  );
}
