import { create } from "zustand";
import type { LaunchMode, LaunchTarget } from "@/lib/ipc";

type PlayBarState = {
  destination: string;
  server: string;
  mode: LaunchMode;
  /** A new destination drops the old server pick; callers that pick one set it afterwards. */
  setDestination: (destination: string) => void;
  setServer: (server: string) => void;
  setMode: (mode: LaunchMode) => void;
};

/** Play bar destination (docs/contracts/discover.md). Discover writes it; the Play bar reads it. */
export const usePlayBar = create<PlayBarState>((set) => ({
  destination: "",
  server: "",
  mode: "any",
  setDestination: (destination) => set({ destination, server: "", mode: "any" }),
  setServer: (server) => set({ server }),
  setMode: (mode) => set({ mode }),
}));

/** Place id from a bare id or a roblox.com/games/<id> link; null otherwise. */
export function placeIdOf(s: string): number | null {
  const m = s.trim().match(/^(\d+)$|^(?:https?:\/\/)?(?:[\w-]+\.)?roblox\.com\/games\/(\d+)/i);
  return m ? Number(m[1] ?? m[2]) : null;
}

const LAST_SET = "rmi.lastLaunchSet";

/** Account ids of the last launch (Home's "Play N accounts"). */
export function lastLaunchSet(): string[] {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(LAST_SET) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

export function saveLaunchSet(ids: string[]) {
  try {
    localStorage.setItem(LAST_SET, JSON.stringify(ids));
  } catch {
    /* storage unavailable: Home falls back to the current selection */
  }
}

/** The Play bar as an account's saved destination: a place id or game link, a private server or
 * share link, plus the JobId when the mode is Specific. Null for anything else (@user, search text). */
export function targetFromBar(
  destination: string,
  server: string,
  mode: LaunchMode,
): LaunchTarget | null {
  const d = destination.trim();
  const placeId = placeIdOf(d);
  const privateLink = isPrivateLink(d) ? d : null;
  if (!placeId && !privateLink) return null;
  const s = server.trim();
  if (mode === "specific" && placeId && s && !UUID.test(s)) return null; // typo'd JobId
  const jobId = mode === "specific" && placeId && s ? s : null;
  if (placeId && !privateLink && (mode === "random" || mode === "smallest"))
    return { placeId, jobId, privateLink, mode };
  return { placeId, jobId, privateLink };
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** Private-server or share link on a Roblox host (Rust `parse_link` rule), scheme optional. */
function isPrivateLink(d: string): boolean {
  let u: URL;
  try {
    u = new URL(/^[a-z]+:\/\//i.test(d) ? d : `https://${d}`);
  } catch {
    return false;
  }
  const h = u.hostname.toLowerCase();
  if (!/^https?:$/.test(u.protocol)) return false;
  if (h === "ro.blox.com") return true;
  if (h !== "roblox.com" && !h.endsWith(".roblox.com")) return false;
  return u.searchParams.has("privateServerLinkCode") || /\/share(-links)?$/.test(u.pathname);
}
