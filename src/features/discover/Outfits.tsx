import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { ExternalLink, Search, X } from "lucide-react";
import { useState } from "react";
import { useUi } from "@/app/store";
import { useSettings } from "@/app/useSettings";
import { toast } from "@/components/Toasts";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { openExternal } from "@/features/accounts/AccountMenu";
import { label } from "@/features/accounts/model";
import { errText, useAccounts } from "@/features/accounts/useAccounts";
import { call, type MissingAsset, type Outfit } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import {
  ErrorLine,
  inputCls,
  Kv,
  PanelEmpty,
  PanelTitle,
  pickedCls,
  Quiet,
  Split,
  Thumb,
} from "./shared";

/** `embedded`: inside the Tools sheet (no outer padding), starts on `defaultUser`. */
export function Outfits({
  embedded,
  defaultUser = "",
}: {
  embedded?: boolean;
  defaultUser?: string;
}) {
  const [name, setName] = useState(defaultUser);
  const [user, setUser] = useState(defaultUser);
  const q = useQuery({
    queryKey: ["outfits", user.toLowerCase()],
    queryFn: () => call("outfits_list", { username: user }),
    enabled: !!user,
    staleTime: 5 * 60_000,
    meta: { silent: true },
  });
  const { data: list } = useAccounts();
  const selId = useUi((u) => u.sel.ids[0]);
  const acc = list?.accounts.find((a) => a.id === selId);
  const { data: s } = useSettings();
  const [wearing, setWearing] = useState<number | null>(null);
  const [pick, setPick] = useState<number>();
  const cur = q.data?.outfits.find((o) => o.id === pick) ?? q.data?.outfits[0];
  /** belongs to the account it was worn on: hidden once another account is selected */
  const [missing, setMissing] = useState<{
    accountId: string;
    outfit: string;
    items: MissingAsset[];
  } | null>(null);

  const wear = async (o: Outfit) => {
    if (!acc) return;
    setWearing(o.id);
    setMissing(null);
    try {
      const r = await call("outfit_wear", { outfitId: o.id, accountId: acc.id });
      if (r.missing.length) setMissing({ accountId: acc.id, outfit: o.name, items: r.missing });
      else
        toast(
          s?.ui.showUsernames === false
            ? t("outfits.wornAnon", { outfit: o.name })
            : t("outfits.worn", { outfit: o.name, account: label(acc) }),
        );
    } catch (e) {
      toast(t("outfits.wearFailed", { reason: errText(e) }));
    } finally {
      setWearing(null);
    }
  };

  const wearBtn = (o: Outfit, className?: string) => (
    <Button
      variant={embedded ? "secondary" : "primary"}
      className={clsx("max-w-full min-w-0", className)}
      disabled={!acc || wearing !== null}
      onClick={() => void wear(o)}
    >
      {wearing === o.id ? (
        t("outfits.wearing")
      ) : acc ? (
        <span className="truncate">
          {t("outfits.wearOn")} <span data-pii>{label(acc)}</span>
        </span>
      ) : (
        t("outfits.wear")
      )}
    </Button>
  );

  let body;
  if (!user) body = <Quiet>{t("outfits.prompt")}</Quiet>;
  else if (q.isPending)
    body = (
      <Grid>
        {Array.from({ length: 8 }, (_, i) => (
          <CardSkeleton key={i} />
        ))}
      </Grid>
    );
  else if (q.isError)
    body = <ErrorLine message={errText(q.error)} retry={() => void q.refetch()} />;
  else if (!q.data.outfits.length)
    body = (
      <Quiet>
        <span data-pii>{t("outfits.none", { name: user })}</span>
      </Quiet>
    );
  else
    body = (
      <Grid label={t("outfits.title", { name: user })}>
        {q.data.outfits.map((o) => (
          <li key={o.id} className="flex flex-col">
            {embedded ? (
              <>
                <Thumb src={o.thumbnail} className="aspect-square rounded-panel bg-surface-2" />
                <p className="mt-2.5 truncate text-14 font-medium text-text-1">{o.name}</p>
                {wearBtn(o, "mt-2 w-full")}
              </>
            ) : (
              <button
                type="button"
                aria-pressed={cur?.id === o.id}
                onClick={() => setPick(o.id)}
                className={clsx(pickedCls(cur?.id === o.id), "p-2")}
              >
                <Thumb src={o.thumbnail} className="aspect-square rounded-chip" />
                <span className="mt-2 block truncate text-13 font-medium text-text-1">
                  {o.name}
                </span>
              </button>
            )}
          </li>
        ))}
      </Grid>
    );

  const form = (
    <form
      className={
        embedded ? "mb-5 flex items-center gap-2" : "flex h-11 shrink-0 items-center gap-2 pb-1"
      }
      onSubmit={(e) => {
        e.preventDefault();
        setUser(name.trim());
      }}
    >
      <input
        aria-label={t("outfits.username")}
        placeholder={t("outfits.username")}
        value={name}
        onChange={(e) => setName(e.target.value)}
        className={clsx(inputCls, embedded ? "w-64" : "min-w-0 flex-1")}
        data-pii
      />
      <Button type="submit" disabled={!name.trim()}>
        <Search size={14} strokeWidth={1.75} />
        {t("outfits.show")}
      </Button>
      {embedded && !acc && user && (
        <p className="ml-2 text-13 text-text-3">{t("outfits.needsAccount")}</p>
      )}
    </form>
  );
  const warn = missing && missing.accountId === selId && (
    <div role="status" className="rounded-panel bg-surface-2 p-4">
      <div className="flex items-start gap-3">
        <p className="flex-1 text-14 text-text-1">
          {t("outfits.missing", { n: missing.items.length, outfit: missing.outfit })}
        </p>
        <IconButton
          label={t("error.dismiss")}
          onClick={() => setMissing(null)}
          className="-mt-1 -mr-1"
        >
          <X size={14} strokeWidth={1.75} />
        </IconButton>
      </div>
      <ul className="mt-3 flex flex-col gap-1">
        {missing.items.map((m) => (
          <li key={m.assetId} className="flex h-9 items-center gap-3">
            <span className="flex-1 truncate text-13 text-text-2">{m.name}</span>
            <Button variant="ghost" onClick={() => void openExternal(m.url)}>
              <ExternalLink size={14} strokeWidth={1.75} />
              {t("outfits.openStore")}
            </Button>
          </li>
        ))}
      </ul>
    </div>
  );

  if (embedded)
    return (
      <div className="pb-4">
        {form}
        {warn && <div className="mb-6 max-w-[640px]">{warn}</div>}
        {body}
      </div>
    );
  return (
    <Split
      label={t("outfits.title", { name: user })}
      list={
        <>
          {form}
          <div className="min-h-0 flex-1 overflow-auto pb-2">{body}</div>
        </>
      }
      panel={
        cur ? (
          <>
            <Thumb
              key={cur.id}
              src={cur.thumbnail}
              className="aspect-square w-56 max-h-56 shrink-0 self-center rounded-panel"
            />
            <PanelTitle title={cur.name} sub={t("outfits.title", { name: user })} />
            {acc ? (
              <Kv rows={[[t("outfits.wearOn"), <span data-pii>{label(acc)}</span>]]} />
            ) : (
              <p className="text-13 text-text-3">{t("outfits.needsAccount")}</p>
            )}
            <div className="flex">{wearBtn(cur)}</div>
            {warn}
          </>
        ) : (
          <PanelEmpty>{t("discover.pick")}</PanelEmpty>
        )
      }
    />
  );
}

function Grid({ children, label: l }: { children: React.ReactNode; label?: string }) {
  return (
    <ul aria-label={l} className="grid grid-cols-[repeat(auto-fill,minmax(120px,1fr))] gap-3">
      {children}
    </ul>
  );
}

function CardSkeleton() {
  return (
    <li aria-hidden className="animate-pulse">
      <div className="aspect-square rounded-panel bg-[var(--skeleton)]" />
      <div className="mt-3 h-3 w-2/3 rounded-full bg-[var(--skeleton)]" />
      <div className="mt-3 h-8 rounded-control bg-[var(--skeleton)]" />
    </li>
  );
}
