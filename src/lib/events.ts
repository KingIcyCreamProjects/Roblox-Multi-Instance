// Tauri event + opener helpers. Outside Tauri (dev browser, tests) they use the mock bus / fall back.
import { inTauri } from "./ipc";

/** The one backend event subscription (mock bus outside Tauri); kept here for old imports. */
export { onEvent } from "./ipc";

/** Opens an https link in the browser; anything else is ignored. */
export async function openExternal(url: string) {
  if (!url.startsWith("https://")) return;
  if (inTauri) await (await import("@tauri-apps/plugin-opener")).openUrl(url);
  else window.open(url, "_blank", "noopener");
}
