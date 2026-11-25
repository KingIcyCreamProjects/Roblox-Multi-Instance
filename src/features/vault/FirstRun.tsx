import { useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { useState, type FormEvent } from "react";
import { useSetSettings } from "@/app/useSettings";
import { Button } from "@/components/ui/Button";
import { call } from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { GateHead } from "./GateHead";
import { PasswordField } from "./PasswordField";
import { vaultError } from "./useVault";

type Mode = "password" | "os";

const options: { mode: Mode; label: MsgKey; hint: MsgKey }[] = [
  { mode: "password", label: "vault.mode.password", hint: "vault.mode.passwordHint" },
  { mode: "os", label: "vault.mode.os", hint: "vault.mode.osHint" },
];

// Length-based on purpose; real guessing resistance comes from Argon2.
// Code points, matching Rust's chars().count().
const len = (p: string) => [...p].length;

function strength(p: string): MsgKey {
  const n = len(p);
  if (n < 8) return "vault.strength.short";
  if (n < 12) return "vault.strength.ok";
  return n < 16 ? "vault.strength.good" : "vault.strength.strong";
}

export function FirstRun() {
  const qc = useQueryClient();
  const { mutate: setSettings } = useSetSettings();
  const [mode, setMode] = useState<Mode>("password");
  const [pw, setPw] = useState("");
  const [confirm, setConfirm] = useState("");
  const [touched, setTouched] = useState(false);
  const [pending, setPending] = useState(false);
  const [serverErr, setServerErr] = useState<string | null>(null);
  // Password lives only in local state (not the mutation cache) and is cleared on success.
  const create = async () => {
    setPending(true);
    setServerErr(null);
    try {
      await call("vault_create", { mode, password: mode === "password" ? pw : null });
      // the ToS line is shown under Create: the Shell banner would repeat it
      setSettings({ onboarding: { tosSeen: true } });
      setPw("");
      setConfirm("");
      setTouched(false);
    } catch (e) {
      setServerErr(vaultError(e));
    } finally {
      setPending(false);
      // also on failure: e.g. "A vault already exists." means the gate should move on
      await qc.invalidateQueries({ queryKey: ["vault"] });
    }
  };

  const usePw = mode === "password";
  const shortErr = touched && usePw && len(pw) < 8 ? t("vault.err.short") : null;
  const matchErr = touched && usePw && !shortErr && pw !== confirm ? t("vault.err.mismatch") : null;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (pending || (usePw && (len(pw) < 8 || pw !== confirm))) return;
    void create();
  };

  return (
    <main className="grid min-h-full place-items-center p-6">
      <form onSubmit={submit} className="flex w-full max-w-[25rem] flex-col gap-3.5" noValidate>
        <GateHead title={t("vault.first.title")}>
          <p className="-mt-1.5 mb-1.5 text-12 text-text-3">{t("vault.first.body")}</p>
        </GateHead>

        <fieldset className="flex flex-col gap-1.5">
          <legend className="sr-only">{t("vault.first.mode")}</legend>
          {options.map((o) => (
            <label
              key={o.mode}
              className={clsx(
                "flex cursor-pointer items-center gap-3 rounded-control px-3.5 py-2.5 transition-colors duration-[var(--dur-quick)]",
                "has-[:focus-visible]:shadow-[var(--focus-ring)]",
                mode === o.mode
                  ? "bg-surface-3 shadow-[inset_0_0_0_1px_var(--hairline-strong)]"
                  : "hover:bg-surface-1",
              )}
            >
              <input
                type="radio"
                name="mode"
                value={o.mode}
                checked={mode === o.mode}
                onChange={() => {
                  setMode(o.mode);
                  setServerErr(null); // an error from the other mode no longer applies
                }}
                className="size-4 accent-[var(--accent)] focus-visible:shadow-none"
              />
              <span className="flex flex-col">
                <span className="text-13 font-semibold text-text-1">{t(o.label)}</span>
                <span className="text-12 text-text-3">{t(o.hint)}</span>
              </span>
            </label>
          ))}
        </fieldset>

        {usePw && (
          <div className="flex flex-col gap-3">
            <PasswordField
              showLabel
              label={t("vault.password")}
              autoComplete="new-password"
              value={pw}
              onChange={(e) => setPw(e.target.value)}
              error={shortErr}
              hint={pw ? t(strength(pw)) : undefined}
              autoFocus
            />
            <PasswordField
              showLabel
              label={t("vault.confirm")}
              autoComplete="new-password"
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
              error={matchErr ?? serverErr}
            />
          </div>
        )}
        {!usePw && serverErr && (
          <p role="alert" className="text-13 text-error">
            {serverErr}
          </p>
        )}

        <Button type="submit" variant="primary" disabled={pending} className="h-9">
          {pending ? t("vault.creating") : t("vault.create")}
        </Button>

        <p className="text-center text-12 text-text-3">{t("tos.note")}</p>
      </form>
    </main>
  );
}
