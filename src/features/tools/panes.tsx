import { useQueryClient } from "@tanstack/react-query";
import {
  cloneElement,
  useId,
  useState,
  type FormEvent,
  type ReactElement,
  type ReactNode,
} from "react";
import { Button } from "@/components/ui/Button";
import { KEY } from "@/features/accounts/useAccounts";
import { call } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { BatchView, Status, useAction, useBatchAction, useTools } from "./useTool";

export const inp =
  "h-9 w-full min-w-0 rounded-control bg-field px-3 text-13 text-text-1 shadow-[inset_0_0_0_1px_var(--hairline-strong)] transition-shadow placeholder:text-text-3 hover:shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus-visible:shadow-[var(--focus-ring)] disabled:opacity-40";
export const USER = /^[A-Za-z0-9_]{3,20}$/;

export function Form({ onSubmit, children }: { onSubmit: () => void; children: ReactNode }) {
  return (
    <form
      className="flex max-w-[420px] flex-col gap-4"
      onSubmit={(e: FormEvent) => {
        e.preventDefault();
        onSubmit();
      }}
    >
      {children}
    </form>
  );
}

export function Field({
  label: l,
  hint,
  error,
  children,
}: {
  label: string;
  hint?: string;
  error?: string | false;
  children: ReactElement<Record<string, unknown>>;
}) {
  const id = useId();
  const note = error || hint;
  return (
    <div className="flex flex-col gap-1.5">
      <label className="flex flex-col gap-1.5">
        <span className="text-13 font-medium text-text-1">{l}</span>
        {cloneElement(children, {
          "aria-describedby": note ? id : undefined,
          "aria-invalid": error ? true : undefined,
        })}
      </label>
      {note && (
        <p
          id={id}
          role={error ? "alert" : undefined}
          className={error ? "text-12 text-error" : "text-12 text-text-3"}
        >
          {note}
        </p>
      )}
    </div>
  );
}

export const Go = ({ disabled, children }: { disabled: boolean; children: ReactNode }) => (
  <Button type="submit" variant="primary" className="max-w-full self-start" disabled={disabled}>
    {children}
  </Button>
);

export const ids = (a: { id: string }[]) => a.map((x) => x.id);

export function DisplayName() {
  const { accounts } = useTools();
  const qc = useQueryClient();
  const [name, setName] = useState("");
  const b = useBatchAction();
  const n = name.trim();
  const bad = name !== "" && (n.length < 3 || n.length > 20);
  return (
    <Form
      onSubmit={() =>
        void b
          .run((batchId) =>
            call("tool_display_name", { batchId, accountIds: ids(accounts), name: n }),
          )
          .then(() => qc.invalidateQueries({ queryKey: KEY }))
      }
    >
      <Field label={t("tools.dn.label")} hint={t("tools.dn.hint")} error={bad && t("tools.dn.bad")}>
        <input
          className={inp}
          value={name}
          onChange={(e) => setName(e.target.value)}
          maxLength={30}
          autoComplete="off"
          spellCheck={false}
        />
      </Field>
      <Go disabled={b.busy || !n || bad}>{t("tools.dn.cta")}</Go>
      <BatchView b={b} />
    </Form>
  );
}

export function Password() {
  const { accounts } = useTools();
  const [cur, setCur] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const a = useAction();
  const short = next !== "" && next.length < 8;
  const same = next !== "" && next === cur;
  const mismatch = again !== "" && again !== next;
  const ok = !!cur && next.length >= 8 && !same && again === next;
  return (
    <Form
      onSubmit={() =>
        void a.run(
          () => call("tool_password", { accountId: accounts[0].id, current: cur, next }),
          t("tools.pw.ok"),
          () => {
            setCur("");
            setNext("");
            setAgain("");
          },
        )
      }
    >
      <Field label={t("tools.pw.current")}>
        <input
          className={inp}
          type="password"
          autoComplete="current-password"
          value={cur}
          onChange={(e) => setCur(e.target.value)}
        />
      </Field>
      <Field
        label={t("tools.pw.new")}
        error={(short && t("tools.pw.short")) || (same && t("tools.pw.same"))}
      >
        <input
          className={inp}
          type="password"
          autoComplete="new-password"
          value={next}
          onChange={(e) => setNext(e.target.value)}
        />
      </Field>
      <Field label={t("tools.pw.confirm")} error={mismatch && t("tools.pw.mismatch")}>
        <input
          className={inp}
          type="password"
          autoComplete="new-password"
          value={again}
          onChange={(e) => setAgain(e.target.value)}
        />
      </Field>
      <Go disabled={a.busy || !ok}>{t("tools.pw.cta")}</Go>
      <Status s={a.s} />
    </Form>
  );
}
