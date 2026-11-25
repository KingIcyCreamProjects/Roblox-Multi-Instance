import * as Tooltip from "@radix-ui/react-tooltip";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { MotionConfig } from "motion/react";
import { useEffect, useRef, type ReactNode } from "react";
import { Button } from "@/components/ui/Button";
import { useAccountEvents } from "@/features/accounts/useAccounts";
import { useSecretToast } from "@/features/automation/useSecretToast";
import { useUpdateLog } from "@/features/updates/UpdateLog";
import { useLaunchEvents } from "@/features/playbar/usePlay";
import { FirstRun } from "@/features/vault/FirstRun";
import { Unlock } from "@/features/vault/Unlock";
import { useLock, useVaultStatus } from "@/features/vault/useVault";
import { heldToast, useToasts } from "@/components/Toasts";
import { ACTIVITY_EVENT, call, inTauri, onEvent, TRAY_HINT, VAULT_LOCKED_EVENT } from "@/lib/ipc";
import { logError } from "@/lib/report";
import { t } from "@/lib/i18n";
import { Shell, useApplyAppearance } from "./Shell";
import { keyLetter, useUi } from "./store";
import { TitleBar } from "./TitleBar";
import { useSettings } from "./useSettings";
import { useStateSync } from "./useStateSync";

// ui.reducedMotion → motion/react: "on" always reduces, "off" never, "system" follows the OS.
const MOTION = { on: "always", off: "never", system: "user" } as const;

const TOUCH_MS = 30_000;

/** Gate screens have no Shell, so give the frameless window a drag region and controls. */
function GateFrame({ children }: { children: ReactNode }) {
  const { data: caps } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  return (
    <div className="app-win flex flex-col">
      <div className="grain" aria-hidden />
      <TitleBar os={caps?.os} minimal />
      <div className="min-h-0 flex-1 overflow-auto">{children}</div>
    </div>
  );
}

/** Routes by vault state; owns lock shortcut, idle-touch and the locked event. */
export function App() {
  const qc = useQueryClient();
  const lock = useLock();
  const { data: v, isError, refetch } = useVaultStatus();
  useApplyAppearance();
  // App-wide backend listeners live here, not in the Shell, so they survive a lock.
  useLaunchEvents();
  useAccountEvents();
  useSecretToast();
  useStateSync();
  const unlocked = !!v?.unlocked;
  // Close-to-tray hint: held so a locked first show keeps it until the Shell's Toaster mounts.
  useEffect(() => onEvent(TRAY_HINT, () => void heldToast(t("tray.hint"))), []);
  // Locked: no Toaster, so listeners' toasts (e.g. launches failing from the lock's cancel_all)
  // are dropped, and the ones on screen at lock time don't come back on unlock.
  const wasUnlocked = useRef(false);
  useEffect(() => {
    if (!v) return;
    useToasts.setState(
      unlocked
        ? { locked: false }
        : wasUnlocked.current
          ? { locked: true, items: [] }
          : { locked: true },
    );
    // the banner queue too: lock-time failures must not greet the user after unlock
    if (!unlocked && wasUnlocked.current) {
      // modals too: dropped cookie text must not sit in the store or reopen after unlock
      useUi.setState((u) => ({
        lastError: null,
        moreErrors: [],
        add: { ...u.add, open: false, prefill: "", prefillFor: null },
        importOpen: false,
        toolsOpen: false,
      }));
      useUpdateLog.setState({ open: false, pick: null });
    }
    wasUnlocked.current = unlocked;
  }, [v, unlocked]);

  // Passwordless auto-unlock is allowed only for the vault state seen at app startup.
  const autoLeft = useRef<boolean | null>(null);
  if (v && autoLeft.current === null) {
    autoLeft.current = v.exists && !v.unlocked && (v.mode === "os" || v.rememberActive);
  }

  // Locked: drop account data from the cache (any lock path: event, command, shortcut), and
  // what Rust hides while locked (Run here / Run on: paired computers and where accounts run).
  useEffect(() => {
    if (unlocked) return;
    qc.removeQueries({
      predicate: (q) =>
        ["accounts", "avatar_full", "tool", "pin", "outfits", "elsewhere", "targets"].includes(
          String(q.queryKey[0]),
        ),
    });
  }, [qc, unlocked]);

  useEffect(() => {
    const refetch = () => void qc.invalidateQueries({ queryKey: ["vault"] });
    // Any command failing with "The vault is locked." (see ipc.call) means our status is stale.
    window.addEventListener(VAULT_LOCKED_EVENT, refetch);
    let off: (() => void) | undefined;
    let dead = false;
    if (inTauri)
      import("@tauri-apps/api/event")
        .then(({ listen }) => listen("vault://locked", refetch))
        .then((f) => {
          if (dead) return f();
          off = f;
          refetch(); // a lock that fired before the listener attached
        })
        .catch((e) => logError(`vault://locked listener failed: ${String(e)}`));
    return () => {
      dead = true;
      off?.();
      window.removeEventListener(VAULT_LOCKED_EVENT, refetch);
    };
  }, [qc]);

  useEffect(() => {
    if (!unlocked) return;
    let last = Date.now();
    const onActivity = (e: Event) => {
      if (e instanceof KeyboardEvent && keyLetter(e) === "l" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        void lock();
        return;
      }
      const now = Date.now();
      if (now - last < TOUCH_MS) return;
      last = now;
      call("vault_touch").catch((err) => logError(`vault_touch failed: ${String(err)}`));
    };
    // mousemove/wheel/scroll fire often; the TOUCH_MS check above throttles them.
    // ACTIVITY_EVENT: keys typed into a remote session, which never reach the window
    const events = ["pointerdown", "keydown", "mousemove", "wheel", ACTIVITY_EVENT] as const;
    for (const ev of events) window.addEventListener(ev, onActivity, { passive: ev !== "keydown" });
    window.addEventListener("scroll", onActivity, { capture: true, passive: true });
    return () => {
      for (const ev of events) window.removeEventListener(ev, onActivity);
      window.removeEventListener("scroll", onActivity, { capture: true });
    };
  }, [unlocked, lock]);

  let body: ReactNode;
  if (!v) {
    // Loading: blank surface, no spinner flash. Error: say so, offer Retry.
    body = isError ? (
      <GateFrame>
        <main className="grid min-h-full place-items-center p-6">
          <div role="alert" className="flex flex-col items-start gap-4">
            <p className="text-14 text-text-1">{t("vault.status.err")}</p>
            <Button variant="primary" onClick={() => void refetch()}>
              {t("vault.retry")}
            </Button>
          </div>
        </main>
      </GateFrame>
    ) : (
      <div className="app-win" />
    );
  } else if (!v.exists) {
    body = (
      <GateFrame>
        <FirstRun />
      </GateFrame>
    );
  } else if (!v.unlocked) {
    body = (
      <GateFrame>
        <Unlock
          status={v}
          auto={!!autoLeft.current}
          onAutoDone={() => (autoLeft.current = false)}
        />
      </GateFrame>
    );
  } else {
    body = <Shell />;
  }
  const { data: s } = useSettings();
  const reduced = MOTION[s?.ui.reducedMotion as keyof typeof MOTION] ?? "user";
  return (
    <MotionConfig reducedMotion={reduced}>
      <Tooltip.Provider delayDuration={400}>{body}</Tooltip.Provider>
    </MotionConfig>
  );
}
