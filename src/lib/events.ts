// Tauri event + opener helpers. Outside Tauri (dev browser, tests) they are no-ops / fall back.
import { inTauri } from "./ipc";

/** Subscribe to a backend event; returns an unsubscribe function. No-op outside Tauri. */
export function onEvent<T = unknown>(name: string, cb: (payload: T) => void): () => void {
  if (!inTauri) return () => {};
  let off: (() => void) | undefined;
  let dead = false;
  void import("@tauri-apps/api/event")
    .then(({ listen }) =>
      listen<T>(name, (e) => cb(e.payload)).then((u) => (dead ? u() : (off = u))),
    )
    .catch((e) => console.error(`listen ${name} failed: ${String(e)}`));
  return () => {
    dead = true;
    off?.();
  };
}

/** Opens an https link in the browser; anything else is ignored. */
export async function openExternal(url: string) {
  if (!url.startsWith("https://")) return;
  if (inTauri) await (await import("@tauri-apps/plugin-opener")).openUrl(url);
  else window.open(url, "_blank", "noopener");
}
