import * as Tabs from "@radix-ui/react-tabs";
import { useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { CircleAlert, CircleCheck, LogIn } from "lucide-react";
import { useId, useState } from "react";
import { useUi, type AddTab } from "@/app/store";
import { toast } from "@/components/Toasts";
import { Button } from "@/components/ui/Button";
import { Sheet } from "@/components/ui/Sheet";
import { call, type AddResult } from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { errText, KEY, useAccountActions } from "./useAccounts";

const TABS: [AddTab, MsgKey][] = [
  ["signin", "add.tab.signin"],
  ["cookies", "add.tab.cookies"],
  ["pending", "add.tab.pending"],
  ["import", "add.tab.import"],
];

const area =
  "min-h-[220px] w-full resize-y rounded-control bg-field shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus:shadow-[var(--focus-ring)] p-3 font-mono text-12 leading-relaxed text-text-1 placeholder:font-sans placeholder:text-13 placeholder:text-text-3";

export function AddSheet() {
  const { open, tab, prefill, prefillFor, nonce } = useUi((u) => u.add);
  const set = (p: Partial<{ open: boolean; tab: AddTab }>) =>
    useUi.setState((u) => ({ add: { ...u.add, ...p } }));
  return (
    <Sheet
      open={open}
      onOpenChange={(o) => set({ open: o })}
      title={t("add.title")}
      description={t("add.body")}
      width={640}
      centered
    >
      <Tabs.Root value={tab} onValueChange={(v) => set({ tab: v as AddTab })}>
        <Tabs.List
          aria-label={t("add.title")}
          className="flex gap-0.5 rounded-control bg-surface-1 p-0.5"
        >
          {TABS.map(([v, k]) => (
            <Tabs.Trigger
              key={v}
              value={v}
              className="h-7 flex-1 rounded-chip px-2 text-12 font-medium whitespace-nowrap text-text-2 transition-colors hover:text-text-1 data-[state=active]:bg-surface-4 data-[state=active]:text-text-1"
            >
              {t(k)}
            </Tabs.Trigger>
          ))}
        </Tabs.List>
        <Tabs.Content value="signin" className="pt-5">
          <SignIn />
        </Tabs.Content>
        <Tabs.Content value="cookies" forceMount className="pt-5 data-[state=inactive]:hidden">
          <Bulk kind="cookies" nonce={nonce} initial={prefillFor === "cookies" ? prefill : ""} />
        </Tabs.Content>
        <Tabs.Content value="pending" forceMount className="pt-5 data-[state=inactive]:hidden">
          <Bulk kind="pending" nonce={nonce} initial={prefillFor === "pending" ? prefill : ""} />
        </Tabs.Content>
        <Tabs.Content value="import" className="pt-5">
          <p className="text-13 text-text-2">{t("add.import.body")}</p>
          <Button
            className="mt-4"
            onClick={() => {
              set({ open: false });
              useUi.setState({ importOpen: true });
            }}
          >
            {t("add.import.cta")}
          </Button>
        </Tabs.Content>
      </Tabs.Root>
    </Sheet>
  );
}

function SignIn() {
  const id = useId();
  const pwId = useId();
  const [hint, setHint] = useState("");
  const [savePw, setSavePw] = useState(false);
  // local only: handed to signin_start once, then cleared
  const [pw, setPw] = useState("");
  const waiting = useUi((u) => u.signinWaiting);
  const { signIn } = useAccountActions();
  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        const password = savePw && pw ? pw : null;
        setPw("");
        void signIn(null, hint.trim() || null, password);
      }}
    >
      <p className="text-13 text-text-2">{t("add.signin.body")}</p>
      <div className="flex flex-col gap-1.5">
        <label htmlFor={id} className="text-13 font-medium text-text-1">
          {t("add.signin.hint")}
        </label>
        <input
          id={id}
          value={hint}
          onChange={(e) => setHint(e.target.value)}
          maxLength={20}
          autoComplete="off"
          spellCheck={false}
          placeholder={t("add.signin.hintPlaceholder")}
          className="h-9 rounded-control bg-field shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus:shadow-[var(--focus-ring)] px-3 text-14 text-text-1 placeholder:text-text-3"
        />
      </div>
      <label className="flex items-center gap-2 text-13 text-text-1">
        <input
          type="checkbox"
          checked={savePw}
          onChange={(e) => {
            setSavePw(e.target.checked);
            if (!e.target.checked) setPw("");
          }}
        />
        {t("add.signin.savePw")}
      </label>
      {savePw && (
        <div className="flex flex-col gap-1.5">
          <label htmlFor={pwId} className="text-13 font-medium text-text-1">
            {t("add.signin.pw")}
          </label>
          <input
            id={pwId}
            type="password"
            value={pw}
            onChange={(e) => setPw(e.target.value)}
            autoComplete="off"
            spellCheck={false}
            className="h-9 rounded-control bg-field shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus:shadow-[var(--focus-ring)] px-3 text-14 text-text-1"
          />
          <p className="text-12 text-text-3">{t("add.signin.pwNote")}</p>
        </div>
      )}
      <Button
        type="submit"
        variant="primary"
        className="self-start"
        // a ticked, empty password would save nothing without saying so
        disabled={waiting || (savePw && !pw)}
      >
        <LogIn size={16} strokeWidth={2} />
        {waiting ? t("add.signin.waiting") : t("empty.signIn")}
      </Button>
      {waiting && (
        <p role="status" className="text-13 text-text-2">
          {t("add.signin.waitingBody")}
        </p>
      )}
    </form>
  );
}

const statusKey: Record<AddResult["status"], MsgKey> = {
  added: "add.res.added",
  updated: "add.res.updated",
  invalid: "add.res.invalid",
  skipped: "add.res.skipped",
};

/** Added/updated lines leave the text (secrets are in the vault now); rejected ones stay to
 * fix, renumbered to where they now are. Done rows get line 0 (no number shown). */
export function afterAdd(text: string, r: AddResult[]) {
  const done = new Set(
    r.filter((x) => x.status === "added" || x.status === "updated").map((x) => x.line),
  );
  const now = new Map<number, number>();
  const kept = text.split(/\r?\n/).filter((l, i) => {
    const keep = !!l.trim() && !done.has(i + 1);
    if (keep) now.set(i + 1, now.size + 1);
    return keep;
  });
  return {
    text: kept.join("\n"),
    results: r.map((x) => ({ ...x, line: now.get(x.line) ?? 0 })),
  };
}

/** Paste cookies / username:password: per-line results inline, sheet stays open. */
function Bulk({
  kind,
  initial,
  nonce,
}: {
  kind: "cookies" | "pending";
  initial: string;
  nonce: number;
}) {
  const id = useId();
  const qc = useQueryClient();
  const [text, setText] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [results, setResults] = useState<AddResult[] | null>(null);
  // a new open/drop resets the form, but never mid-submit (submit merges it when done)
  const [seen, setSeen] = useState(nonce);
  if (nonce !== seen && !busy) {
    setSeen(nonce);
    setText(initial);
    setErr(null);
    setResults(null);
  }
  const submit = async () => {
    setBusy(true);
    setErr(null);
    let rest = text;
    try {
      const r = await call(kind === "cookies" ? "accounts_add_cookies" : "accounts_add_pending", {
        text,
      });
      const a = afterAdd(text, r);
      rest = a.text;
      setResults(a.results);
      const n = r.filter((x) => x.status === "added").length;
      if (n) toast(n === 1 ? t("add.toast1") : t("add.toast", { n }));
      void qc.invalidateQueries({ queryKey: KEY });
    } catch (e) {
      setErr(errText(e));
      setResults(null);
    } finally {
      // text dropped while busy is kept, after what is left of this batch
      const add = useUi.getState().add;
      const dropped = add.nonce !== nonce && add.prefillFor === kind ? add.prefill : "";
      setText([rest, dropped].filter(Boolean).join("\n"));
      setSeen(add.nonce);
      if (add.prefillFor === kind)
        useUi.setState((u) => ({ add: { ...u.add, prefill: "", prefillFor: null } }));
      setBusy(false);
    }
  };
  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <label htmlFor={id} className="text-13 text-text-2">
        {kind === "cookies" ? t("add.cookies.body") : t("add.pending.body")}
      </label>
      <textarea
        id={id}
        value={text}
        onChange={(e) => setText(e.target.value)}
        readOnly={busy}
        spellCheck={false}
        autoComplete="off"
        autoFocus
        placeholder={
          kind === "cookies" ? t("add.cookies.placeholder") : t("add.pending.placeholder")
        }
        className={area}
      />
      {err && (
        <p role="alert" className="text-13 text-error">
          {err}
        </p>
      )}
      <Button
        type="submit"
        variant="primary"
        className="self-start"
        disabled={busy || !text.trim()}
      >
        {busy ? t("add.adding") : t("add.cta")}
      </Button>
      {results && (
        <ul aria-label={t("add.results")} className="mt-2 flex flex-col">
          {results.length === 0 && <li className="text-13 text-text-3">{t("add.res.none")}</li>}
          {results.map((r, i) => {
            const ok = r.status === "added" || r.status === "updated";
            return (
              <li
                key={i}
                className="flex items-start gap-2.5 border-t border-hairline py-2 text-13 first:border-t-0"
              >
                {ok ? (
                  <CircleCheck
                    size={15}
                    strokeWidth={1.75}
                    className="mt-0.5 shrink-0 text-online"
                  />
                ) : (
                  <CircleAlert
                    size={15}
                    strokeWidth={1.75}
                    className={clsx(
                      "mt-0.5 shrink-0",
                      r.status === "invalid" ? "text-error" : "text-text-3",
                    )}
                  />
                )}
                <span className="w-14 shrink-0 text-text-3 tnum">
                  {r.line > 0 && t("add.res.line", { n: r.line })}
                </span>
                <span className="min-w-0 flex-1 text-text-1">
                  {t(statusKey[r.status])}
                  {r.username && (
                    <span data-pii className="text-text-2">
                      {" "}
                      @{r.username}
                    </span>
                  )}
                  {r.message && <span className="block text-text-2">{r.message}</span>}
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </form>
  );
}
