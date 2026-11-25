import { useQuery, useQueryClient } from "@tanstack/react-query";
import { TriangleAlert } from "lucide-react";
import { useEffect } from "react";
import { call, errMessage, onEvent } from "@/lib/ipc";
import { namesHidden, useAccounts } from "@/features/accounts/useAccounts";
import { t } from "@/lib/i18n";
import { AutomationPage } from "@/features/automation/AutomationPage";
import { toast, Toaster } from "@/components/Toasts";
import { Button } from "@/components/ui/Button";
import { AccountSheets, AccountsScreen } from "@/features/accounts/AccountsScreen";
import { ReauthSheet } from "@/features/accounts/Reauth";
import { Discover } from "@/features/discover/Discover";
import { DevicesPage } from "@/features/devices/DevicesPage";
import { Home } from "@/features/home/Home";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { PlayBar, RunningBar } from "@/features/playbar/PlayBar";
import { useLaunch } from "@/features/playbar/usePlay";
import { Tour, tourSteps } from "@/features/onboarding/Tour";
import { UpdateLog, useWhatsNew } from "@/features/updates/UpdateLog";
import { Palette } from "./Palette";
import { Tabs, tabOrder } from "./Tabs";
import { TitleBar } from "./TitleBar";
import { useSettings, useSetSettings } from "./useSettings";
import { keyLetter, useUi } from "./store";

export function useApplyAppearance() {
  const { data: s } = useSettings();
  useEffect(() => {
    if (!s) return;
    const root = document.documentElement;
    const apply = () => {
      const theme =
        s.theme === "system"
          ? matchMedia("(prefers-color-scheme: light)").matches
            ? "light"
            : "dark"
          : s.theme;
      if (root.dataset.theme !== theme) {
        // one frame without colour transitions so the switch does not animate
        root.dataset.themeSwitching = "";
        root.dataset.theme = theme;
        requestAnimationFrame(() =>
          requestAnimationFrame(() => delete root.dataset.themeSwitching),
        );
      }
      try {
        localStorage.setItem("rmi.theme", theme); // read by main.tsx before first paint
      } catch {
        /* storage unavailable: next start may flash, nothing else */
      }
    };
    apply();
    root.dataset.density = s.ui.density;
    root.dataset.privacy = s.ui.showUsernames ? "off" : "on";
    root.dataset.reducedMotion = s.ui.reducedMotion;
    root.style.setProperty("--ui-scale", String(s.ui.scale / 100));
    const mq = matchMedia("(prefers-color-scheme: light)");
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [s]);
}

function TosNote() {
  const { data: s } = useSettings();
  const set = useSetSettings();
  if (!s || s.onboarding.tosSeen) return null;
  return (
    <div
      role="note"
      className="glass-panel mx-7 mb-2 flex items-center gap-4 rounded-panel px-4 py-2 text-13 text-text-2"
    >
      <p className="flex-1">{t("tos.note")}</p>
      <Button variant="ghost" sm onClick={() => set.mutate({ onboarding: { tosSeen: true } })}>
        {t("tos.dismiss")}
      </Button>
    </div>
  );
}

export function ErrorBanner({
  queryError,
  retry,
}: {
  queryError?: Error | null;
  retry: () => void;
}) {
  const lastError = useUi((u) => u.lastError);
  const setError = useUi((u) => u.setError);
  const more = useUi((u) => u.moreErrors.length);
  const msg = queryError ? errMessage(queryError) : lastError;
  if (!msg) return null;
  return (
    <div
      role="alert"
      className="glass-panel mx-7 mb-2 flex items-center gap-3 rounded-panel px-4 py-2 text-13 text-text-2"
    >
      <TriangleAlert size={14} strokeWidth={1.75} className="shrink-0 text-error" aria-hidden />
      <p className="min-w-0 flex-1 break-words">{msg}</p>
      {!queryError && more > 0 && (
        <span className="shrink-0 text-text-3">{t("error.more", { n: more })}</span>
      )}
      <Button variant="ghost" sm onClick={queryError ? retry : () => setError(null)}>
        {queryError ? t("error.retry") : t("error.dismiss")}
      </Button>
    </div>
  );
}

/** App-start route, to tell whether the user moved before settings loaded. */
const start = { done: false, route: useUi.getState().route };

let recoveredShown = false;

export function Shell() {
  // appearance: App applies it (gates included); the Shell always renders inside App
  useWhatsNew();
  // once per app start (a lock/unlock remounts the Shell): corrupt settings were reset
  const { data: info } = useQuery({ queryKey: ["info"], queryFn: () => call("app_info") });
  useEffect(() => {
    if (!info?.settingsRecovered || recoveredShown) return;
    recoveredShown = true;
    toast(info.settingsRecovered, undefined, 12000);
  }, [info]);
  const qc = useQueryClient();
  useEffect(
    () =>
      onEvent<{ action: "disconnect" | "reconnect" | "add"; displayName: string }>(
        "friend_action",
        ({ action, displayName }) => {
          const k = (
            { disconnect: "disconnected", reconnect: "reconnected", add: "added" } as const
          )[action];
          toast(namesHidden(qc) ? t(`friend.${k}Anon`) : t(`friend.${k}`, { name: displayName }));
        },
      ),
    [qc],
  );
  // Drop selection ids that no longer exist (removed elsewhere, refetch), on every route:
  // the Play bar counts and launches the raw selection.
  const { data: list } = useAccounts();
  useEffect(() => {
    if (!list) return;
    const live = new Set(list.accounts.map((a) => a.id));
    const cur = useUi.getState().sel;
    if (cur.ids.some((id) => !live.has(id)) || (cur.cursor && !live.has(cur.cursor)))
      useUi.getState().setSel({
        ids: cur.ids.filter((id) => live.has(id)),
        anchor: cur.anchor && live.has(cur.anchor) ? cur.anchor : null,
        cursor: cur.cursor && live.has(cur.cursor) ? cur.cursor : null,
      });
  }, [list]);
  const { data: s, error: settingsErr } = useSettings();
  const { data: caps, error: capsErr } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  const retry = () => {
    // pop the queued query error only; other queued errors stay for the user
    const qe = settingsErr ?? capsErr;
    if (qe && useUi.getState().lastError === errMessage(qe)) useUi.getState().setError(null);
    void qc.refetchQueries({ queryKey: ["settings"] });
    void qc.refetchQueries({ queryKey: ["caps"] });
  };
  const route = useUi((u) => u.route);
  const running = useLaunch((l) => !!l.batch);
  // no accounts yet: the empty state's Sign in is the only action, no Play bar under it
  const playbar = (route === "accounts" || route === "home") && list?.accounts.length !== 0;
  // Automation switched off (here, by the API or an import) while on its page: leave it.
  const autoOff = s?.automation.enabled === false;
  useEffect(() => {
    if (autoOff && route === "automation") useUi.getState().go("accounts");
  }, [autoOff, route]);
  const { mutate: setSettings } = useSetSettings();

  // Once per app start (Shell remounts on every unlock): open on the first tab of the saved
  // order, unless the user already moved.
  useEffect(() => {
    if (!s || start.done) return;
    start.done = true;
    if (useUi.getState().route !== start.route) return;
    const first = tabOrder(s).find((r) => r !== "automation" || s.automation.enabled);
    if (first) useUi.getState().go(first);
  }, [s]);

  // Ctrl/Cmd+Shift+H toggles privacy mode (same as the title-bar eye).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || keyLetter(e) !== "h" || !s) return;
      e.preventDefault();
      setSettings({ ui: { showUsernames: !s.ui.showUsernames } });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [s, setSettings]);

  return (
    // unlocking fades the app in rather than swapping it in on one frame
    <div className="app-win flex animate-[fade-in_var(--dur-hero)_var(--ease-out)] flex-col">
      <div className="grain" aria-hidden />
      <TitleBar os={caps?.os}>
        <Tabs automation={!!s?.automation.enabled} />
      </TitleBar>
      <div className="flex min-h-0 flex-1">
        <main className="flex min-w-0 flex-1 flex-col">
          <ErrorBanner queryError={settingsErr ?? capsErr} retry={retry} />
          <TosNote />
          <div className="min-h-0 flex-1 overflow-auto">
            {route === "home" ? (
              <Home />
            ) : route === "accounts" ? (
              <AccountsScreen />
            ) : route === "automation" && !autoOff ? (
              <AutomationPage />
            ) : route === "discover" ? (
              <Discover />
            ) : route === "devices" ? (
              <DevicesPage />
            ) : route === "settings" ? (
              <SettingsPage />
            ) : (
              <div className="p-8 text-text-3" />
            )}
          </div>
          {playbar ? <PlayBar /> : running && <RunningBar />}
          <Toaster playbar={playbar || running} />
        </main>
        <Palette />
        <ReauthSheet />
        <AccountSheets />
        <UpdateLog />
        {s && caps && tourSteps(s, caps).length > 0 && <Tour caps={caps} />}
      </div>
    </div>
  );
}
