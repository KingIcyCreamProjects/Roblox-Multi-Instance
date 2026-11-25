import { ShieldCheck } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { create } from "zustand";
import { Button } from "@/components/ui/Button";
import { Sheet } from "@/components/ui/Sheet";
import { PasswordField } from "@/features/vault/PasswordField";
import { useVaultStatus, vaultError } from "@/features/vault/useVault";
import { call } from "@/lib/ipc";
import { t } from "@/lib/i18n";

const useReauth = create<{ resolve: ((ok: boolean) => void) | null }>(() => ({ resolve: null }));

let pending: Promise<boolean> | null = null;

/** Opens the re-auth sheet; resolves true once the vault accepted fresh credentials.
 * A request while the sheet is open shares its answer (it doesn't cancel the first). */
export const requestReauth = () => {
  pending ??= new Promise<boolean>((resolve) =>
    useReauth.setState({
      resolve: (ok) => {
        pending = null;
        resolve(ok);
      },
    }),
  );
  return pending;
};

export function ReauthSheet() {
  const resolve = useReauth((s) => s.resolve);
  const { data: v } = useVaultStatus();
  const [pw, setPw] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const os = v?.mode === "os";
  const { data: caps } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  const osLabel = caps?.os === "windows" ? t("reauth.os") : t("reauth.osOther");
  // Unmounted (lock, route change) with a request open: settle it as cancelled.
  useEffect(
    () => () => {
      useReauth.getState().resolve?.(false);
      useReauth.setState({ resolve: null });
    },
    [],
  );

  const finish = (ok: boolean) => {
    // a late result of a request that was closed and reopened must not settle the new one
    if (useReauth.getState().resolve !== resolve) return;
    resolve?.(ok);
    useReauth.setState({ resolve: null });
    setPw("");
    setErr(null);
  };

  const submit = async () => {
    setBusy(true);
    setErr(null);
    try {
      await call("reauth", { password: os ? null : pw });
      finish(true);
    } catch (e) {
      // closed (or closed and reopened) meanwhile: the old error belongs to no open request
      if (useReauth.getState().resolve === resolve) setErr(vaultError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      open={!!resolve}
      onOpenChange={(o) => !o && finish(false)}
      title={t("reauth.title")}
      description={t("reauth.body")}
      width={380}
      centered
    >
      <form
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        {!os && (
          <PasswordField
            label={t("vault.password")}
            value={pw}
            onChange={(e) => setPw(e.target.value)}
            error={err}
            autoFocus
            autoComplete="current-password"
          />
        )}
        {os && err && (
          <p role="alert" className="text-13 text-error">
            {err}
          </p>
        )}
        <Button type="submit" variant="primary" disabled={busy || (!os && !pw)}>
          {os && <ShieldCheck size={16} strokeWidth={1.75} />}
          {busy ? t("reauth.checking") : os ? osLabel : t("reauth.confirm")}
        </Button>
      </form>
    </Sheet>
  );
}
