import { errMessage, hasCode, inTauri } from "./ipc";
import { useUi } from "@/app/store";

/** Log through the Rust logger (redacted, in the log file) when in Tauri, else the console. */
export function logError(msg: string) {
  if (!inTauri) return console.error(msg);
  import("@tauri-apps/plugin-log").then((m) => m.error(msg)).catch(() => console.error(msg));
}

/** Log and show in the shell's error banner. */
export function reportError(e: unknown) {
  const msg = errMessage(e);
  logError(e instanceof Error ? e.message : String(e));
  // the lock screen already says it; a banner would wait and show after unlock
  if (hasCode(e, "VAULT_LOCKED") || hasCode(e, "VAULT_WAS_LOCKED")) return;
  useUi.getState().setError(msg);
}
