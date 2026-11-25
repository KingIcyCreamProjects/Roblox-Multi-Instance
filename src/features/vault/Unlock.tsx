import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { LockOpen } from "lucide-react";
import { Button } from "@/components/ui/Button";
import { call, hasCode, type VaultStatus } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { GateHead } from "./GateHead";
import { PasswordField } from "./PasswordField";
import { Recovery } from "./Recovery";
import { vaultError } from "./useVault";

/**
 * `auto`: app-startup only. One passwordless attempt (OS-wrapped or remembered vault);
 * the parent clears it via onAutoDone so lock / vault://locked never re-triggers it.
 */
export function Unlock({
  status,
  auto,
  onAutoDone,
}: {
  status: VaultStatus;
  auto: boolean;
  onAutoDone: () => void;
}) {
  const qc = useQueryClient();
  const os = status.mode === "os";
  const [pw, setPw] = useState("");
  const [remember, setRemember] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [damaged, setDamaged] = useState(false);
  const [pending, setPending] = useState(false);
  const input = useRef<HTMLInputElement>(null);

  // Password lives only in local state (not the mutation cache) and is cleared on success.
  const run = async (password: string | null, rem: boolean, silent: boolean) => {
    setPending(true);
    setError(null);
    setDamaged(false);
    try {
      await call("vault_unlock", { password, remember: rem });
      setPw("");
      await qc.invalidateQueries({ queryKey: ["vault"] });
    } catch (e) {
      // VAULT_WAS_LOCKED: a Lock (tray) cancelled this unlock. Lock wins; not an error.
      if (!silent && !hasCode(e, "VAULT_WAS_LOCKED")) {
        setError(vaultError(e));
        setDamaged(hasCode(e, "VAULT_DAMAGED"));
      }
    } finally {
      setPending(false);
    }
  };

  const ran = useRef(false);
  useEffect(() => {
    if (!auto || ran.current) return;
    ran.current = true;
    onAutoDone();
    // Remembered password vault: failure just means the form. OS vault: show why.
    void run(null, false, !os);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!pending) input.current?.focus();
  }, [pending]);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (pending) return;
    if (!os && !pw) return setError(t("vault.err.empty"));
    void run(os ? null : pw, remember, false);
  };

  if (status.damaged)
    return (
      <main className="grid min-h-full place-items-center p-6">
        <div className="flex w-full max-w-[20rem] flex-col gap-4">
          <GateHead title={t("vault.damaged.title")}>
            <p role="alert" className="-mt-1 text-13 text-text-2">
              {t("vault.damaged.body")}
            </p>
          </GateHead>
          <Recovery />
        </div>
      </main>
    );

  return (
    <main className="grid min-h-full place-items-center p-6">
      <div className="flex w-full max-w-[20rem] flex-col gap-4">
        <form onSubmit={submit} className="flex flex-col gap-3.5" noValidate>
          <GateHead title={os ? t("vault.unlock.osTitle") : t("vault.unlock.title")}>
            <p className="-mt-1.5 mb-1.5 text-12 text-text-3">
              {os ? t("vault.unlock.osBody") : t("vault.unlock.body")}
            </p>
          </GateHead>

          {os ? (
            error && (
              <p role="alert" className="text-13 text-error">
                {error}
              </p>
            )
          ) : (
            <>
              <PasswordField
                ref={input}
                label={t("vault.password")}
                autoComplete="current-password"
                value={pw}
                onChange={(e) => {
                  setPw(e.target.value);
                  if (!damaged) setError(null); // a new try: the old "Wrong password." no longer applies
                }}
                error={error}
                disabled={pending}
              />
              <label className="flex cursor-pointer items-center gap-2.5 text-13 text-text-2 hover:text-text-1 has-[:disabled]:cursor-default has-[:disabled]:opacity-40">
                <input
                  type="checkbox"
                  checked={remember}
                  disabled={pending}
                  onChange={(e) => setRemember(e.target.checked)}
                  className="size-4 accent-[var(--accent)]"
                />
                {t("vault.remember")}
              </label>
            </>
          )}

          <Button type="submit" variant="primary" disabled={pending} className="h-9">
            {os && !pending && <LockOpen size={16} strokeWidth={1.75} aria-hidden />}
            {pending ? t("vault.unlocking") : t("vault.unlock")}
          </Button>
        </form>
        {damaged && (
          <Recovery
            onRestored={() => {
              setError(null);
              setDamaged(false);
            }}
          />
        )}
      </div>
    </main>
  );
}
