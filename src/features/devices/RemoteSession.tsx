import { X } from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Button } from "@/components/ui/Button";
import { useUi } from "@/app/store";
import { ACTIVITY_EVENT, inTauri, type RemoteConnect } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { loadViewer } from "./viewer";

/** The viewer's connection dot while its socket is open (hello received). */
const CONNECTED = '.rv-dot[data-tone="ok"], .rv-dot[data-tone="warn"]';

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** The window below the title bar (its drag region and window buttons stay usable) with the
 *  shared viewer mounted. The token goes to the viewer, which sends it as the socket's first
 *  message; it is never put in the URL or logged. `onClose` gets the viewer's bye code. */
export function RemoteSession({
  conn,
  onClose,
  returnFocus,
}: {
  conn: RemoteConnect;
  onClose: (why?: string) => void;
  /** Where focus goes when the session ends (the device's Connect button). */
  returnFocus: () => HTMLElement | null;
}) {
  const root = useRef<HTMLDivElement>(null);
  const mount = useRef<HTMLDivElement>(null);
  const [missing, setMissing] = useState(false);
  const close = useRef(onClose);
  const back = useRef(returnFocus);
  useEffect(() => {
    close.current = onClose;
    back.current = returnFocus;
  });

  // focus leaves the button that opened it (Enter would press it again) and returns there;
  // the tabs stay put meanwhile (switching would silently end the session)
  useEffect(() => {
    root.current?.focus({ preventScroll: true });
    useUi.setState({ remoteOpen: true });
    // a live session counts as activity for the idle auto-lock even when the user only
    // watches it (locking would end the session); 20 s keeps the throttled vault_touch
    // calls in App under the shortest auto-lock time (1 minute). Counts while shown and
    // connected, also on the other computer's lock screen or a terminal-only view: the viewer's
    // status dot is "ok" or "warn" then (warn also covers a brief reconnect), "bad" once the
    // session ended or failed, empty while connecting. A hidden window leaves the idle timer alone.
    const alive = setInterval(() => {
      if (document.visibilityState === "visible" && mount.current?.querySelector(CONNECTED))
        window.dispatchEvent(new Event(ACTIVITY_EVENT));
    }, 20_000);
    return () => {
      clearInterval(alive);
      useUi.setState({ remoteOpen: false });
      back.current()?.focus({ preventScroll: true });
    };
  }, []);

  useEffect(() => {
    // the browser preview's mock peers are fixtures: no socket at all from there
    if (!inTauri) return;
    let handle: { close: () => void } | null = null;
    let gone = false;
    loadViewer().then(
      (v) => {
        if (gone || !mount.current) return;
        handle = v.open({
          mount: mount.current,
          url: conn.url,
          auth: conn.token,
          title: conn.name,
          onClose: (why) => close.current(why),
        });
      },
      () => !gone && setMissing(true),
    );
    return () => {
      gone = true;
      handle?.close();
    };
  }, [conn]);

  // keys typed for the other computer never reach this app's shortcuts (Ctrl+L locks the
  // vault, Ctrl+Shift+H, Ctrl+K): they stop here, before the window listeners, but still
  // count as activity for the idle auto-lock
  const stop = (e: KeyboardEvent) => {
    e.stopPropagation();
    window.dispatchEvent(new Event(ACTIVITY_EVENT));
  };
  // aria-modal: Tab the viewer leaves alone cycles inside the overlay
  const keyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    stop(e);
    if (e.key !== "Tab" || e.nativeEvent.defaultPrevented) return;
    const f = [...e.currentTarget.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
      (el) => !el.closest("[inert]"),
    );
    const [first, last] = [f[0], f[f.length - 1]];
    const at = document.activeElement;
    if (!first || !last) {
      e.preventDefault();
    } else if (e.shiftKey && (at === first || at === e.currentTarget)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && at === last) {
      e.preventDefault();
      first.focus();
    }
  };
  return (
    <div
      ref={root}
      role="dialog"
      aria-modal
      tabIndex={-1}
      aria-label={t("dev.sessionTitle", { name: conn.name })}
      onKeyDown={keyDown}
      onKeyUp={stop}
      className="fixed inset-x-0 top-[var(--titlebar-h)] bottom-0 z-50 flex flex-col bg-bg outline-none"
    >
      {missing || !inTauri ? (
        <div className="m-auto flex max-w-md flex-col items-start gap-3 p-8">
          <p role="alert" className="text-14 text-text-1">
            {t(inTauri ? "dev.viewerMissing" : "dev.previewOnly")}
          </p>
          <Button onClick={() => onClose()}>
            <X size={16} strokeWidth={1.75} aria-hidden />
            {t("sheet.close")}
          </Button>
        </div>
      ) : (
        <div ref={mount} className="min-h-0 flex-1" />
      )}
    </div>
  );
}
