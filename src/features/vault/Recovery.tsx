import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/Button";
import { call, hasCode } from "@/lib/ipc";
import { LOCALE, t } from "@/lib/i18n";
import { logError } from "@/lib/report";
import { PasswordField } from "./PasswordField";
import { vaultError } from "./useVault";

/**
 * Damaged-vault recovery on the locked screen: restore a backup (Rust only allows this
 * while the current file can't be opened, and keeps the damaged file) or open the folder.
 */
export function Recovery({ onRestored }: { onRestored?: () => void } = {}) {
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [pw, setPw] = useState("");
  /** name of the backup being restored */
  const [pending, setPending] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [wrongPw, setWrongPw] = useState(false);
  const { data: backups, isError } = useQuery({
    queryKey: ["vault", "backups"],
    queryFn: () => call("vault_backups"),
    enabled: open,
  });

  const restore = async (name: string) => {
    setPending(name);
    setErr(null);
    setWrongPw(false);
    try {
      await call("vault_restore", { name, password: pw || null });
      setPw("");
      onRestored?.();
      await qc.invalidateQueries({ queryKey: ["vault"] });
      await qc.invalidateQueries({ queryKey: ["accounts"] });
    } catch (e) {
      setErr(vaultError(e));
      setWrongPw(hasCode(e, "WRONG_PASSWORD"));
    } finally {
      setPending(null);
    }
  };

  // shown here: the locked screen has no error banner (reportError would surface after unlock)
  const openFolder = () =>
    call("open_folder", { kind: "backups" }).catch((e: unknown) => {
      logError(`open_folder failed: ${String(e)}`);
      setErr(vaultError(e));
      setWrongPw(false);
    });

  return (
    <section aria-label={t("vault.damaged.restore")} className="flex flex-col gap-4">
      <div className="flex flex-wrap justify-center gap-2">
        <Button variant="primary" onClick={() => setOpen(true)} disabled={open}>
          {t("vault.damaged.restore")}
        </Button>
        <Button onClick={() => void openFolder()}>{t("vault.damaged.openFolder")}</Button>
      </div>
      {err && !wrongPw && (
        <p role="alert" className="text-13 text-error">
          {err}
        </p>
      )}
      {open && (
        <div className="flex flex-col gap-3">
          <PasswordField
            showLabel
            label={t("vault.damaged.pw")}
            autoComplete="current-password"
            value={pw}
            onChange={(e) => {
              setPw(e.target.value);
              if (wrongPw) {
                setErr(null);
                setWrongPw(false);
              }
            }}
            error={wrongPw ? err : null}
            disabled={!!pending}
          />
          {isError && (
            <p role="alert" className="text-13 text-error">
              {t("vs.backups.err")}
            </p>
          )}
          {backups?.length === 0 && <p className="text-13 text-text-3">{t("vs.backups.none")}</p>}
          <ul className="flex flex-col gap-1">
            {backups?.map((b) => (
              <li key={b.name} className="flex items-center gap-3">
                <span className="text-13 text-text-2 tabular-nums">
                  {new Date(b.createdAt).toLocaleString(LOCALE)}
                </span>
                <Button
                  className="ml-auto"
                  disabled={!!pending}
                  aria-label={t("vault.damaged.pickOf", { name: b.name })}
                  onClick={() => void restore(b.name)}
                >
                  {pending === b.name ? t("vs.working") : t("vault.damaged.pick")}
                </Button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
