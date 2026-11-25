import { Copy, Eye, EyeOff, Lock, Megaphone, Minus, Search, Square, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { Mark } from "@/components/Mark";
import { IconButton } from "@/components/ui/IconButton";
import { inTauri } from "@/lib/ipc";
import { logError } from "@/lib/report";
import { t } from "@/lib/i18n";
import { useLock } from "@/features/vault/useVault";
import { openUpdateLog } from "@/features/updates/UpdateLog";
import { useUi } from "./store";
import { useSettings, useSetSettings } from "./useSettings";

async function win() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

type Win = Awaited<ReturnType<typeof win>>;
const winAction = (f: (w: Win) => Promise<unknown>) => async () => {
  if (!inTauri) return;
  try {
    await f(await win());
  } catch (e) {
    logError(`window control: ${String(e)}`);
  }
};

/** `minimal`: vault-gate screens, so only drag region, brand and window controls. */
export function TitleBar({
  os,
  minimal,
  children,
}: {
  os?: string;
  minimal?: boolean;
  /** centre slot: the tab capsule */
  children?: ReactNode;
}) {
  const { data: s } = useSettings();
  const set = useSetSettings();
  const hidden = s ? !s.ui.showUsernames : false;
  const lock = useLock();
  const remoteOpen = useUi((u) => u.remoteOpen);
  // Until platform info loads, guess from the UA so macOS never flashes Windows controls.
  const mac = (os ?? (navigator.userAgent.includes("Mac") ? "macos" : undefined)) === "macos";
  // frameless: our own Maximize button, so it must turn into Restore when maximized
  const [max, setMax] = useState(false);
  useEffect(() => {
    if (!inTauri || mac) return;
    let off: (() => void) | undefined;
    let dead = false;
    void win()
      .then(async (w) => {
        const sync = () => void w.isMaximized().then(setMax, () => {});
        sync();
        const un = await w.onResized(sync);
        if (dead) un();
        else off = un;
      })
      .catch((e: unknown) => logError(`window control: ${String(e)}`));
    return () => {
      dead = true;
      off?.();
    };
  }, [mac]);

  return (
    <header
      data-tauri-drag-region
      data-modal-pass
      className="titlebar @container/bar grid h-[var(--titlebar-h)] shrink-0 grid-cols-[auto_1fr_auto] items-center gap-3 pr-3 pl-5"
      // macOS: clears the traffic lights (trafficLightPosition x 20, three 14 px buttons 20 px apart) + 12 px
      style={mac ? { paddingLeft: 86 } : undefined}
    >
      <div data-tauri-drag-region className="flex min-w-0 items-center gap-2 text-text-2">
        <Mark size={16} className="shrink-0 text-text-2" />
        {/* narrow window or big UI scale: keep the mark only, the tabs need the room
            (58rem = the 960px minimum window at 100%, minus the bar's padding) */}
        <span data-tauri-drag-region className="truncate text-12 @max-[57.9rem]/bar:hidden">
          {t("app.name")}
        </span>
      </div>

      <div data-tauri-drag-region className="@container flex min-w-0 justify-center">
        {children}
      </div>

      <div data-tauri-drag-region className="flex items-center justify-end gap-0.5">
        {!minimal && (
          <>
            {/* a remote session holds the screen: no palette or update log over it */}
            <IconButton
              label={t("title.search")}
              aria-haspopup="dialog"
              disabled={remoteOpen}
              onClick={() => useUi.setState({ paletteOpen: true })}
            >
              <Search size={16} strokeWidth={1.75} />
            </IconButton>
            <IconButton
              label={t("updates.open")}
              aria-haspopup="dialog"
              disabled={remoteOpen}
              onClick={() => openUpdateLog()}
            >
              <Megaphone size={16} strokeWidth={1.75} />
            </IconButton>
            <IconButton
              label={hidden ? t("title.privacyOn") : t("title.privacyOff")}
              onClick={() => set.mutate({ ui: { showUsernames: hidden } })}
            >
              {hidden ? (
                <EyeOff size={16} strokeWidth={1.75} />
              ) : (
                <Eye size={16} strokeWidth={1.75} />
              )}
            </IconButton>
            <IconButton label={t("title.lock")} onClick={lock}>
              <Lock size={16} strokeWidth={1.75} />
            </IconButton>
          </>
        )}
        {!mac && (
          <div className="win-ctl ml-2 flex items-center gap-1">
            <IconButton label={t("title.minimize")} onClick={winAction((w) => w.minimize())}>
              <Minus size={16} strokeWidth={1.75} />
            </IconButton>
            <IconButton
              label={t(max ? "title.restore" : "title.maximize")}
              onClick={winAction((w) => w.toggleMaximize())}
            >
              {max ? (
                <Copy size={13} strokeWidth={1.75} />
              ) : (
                <Square size={13} strokeWidth={1.75} />
              )}
            </IconButton>
            <IconButton
              label={t("title.close")}
              className="hover:bg-error! hover:text-on-image!"
              onClick={winAction((w) => w.close())}
            >
              <X size={16} strokeWidth={1.75} />
            </IconButton>
          </div>
        )}
      </div>
    </header>
  );
}
