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

/** Place id from a bare id or a Roblox game link (same rules as Rust `parse_link`: optional
 * locale, /games/start?placeId=N, ro.blox.com af_web_dp); null otherwise. */
export function placeIdOf(s: string): number | null {
  const d = s.trim();
  return /^\d+$/.test(d) ? Number(d) : (parseLink(d)?.placeId ?? null);
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
  const link = parseLink(d);
  const placeId = placeIdOf(d);
  const privateLink = link?.priv ? d : null;
  if (!placeId && !privateLink) return null;
  // a job link in the server field counts as its JobId (Rust `parse_server`)
  const s = parseLink(server.trim())?.jobId ?? server.trim();
  if (mode === "specific" && placeId && s && !UUID.test(s)) return null; // typo'd JobId
  // a game-instance destination link names its server; Rust launches it whatever the mode
  const jobId = privateLink
    ? null
    : (link?.jobId ?? (mode === "specific" && placeId && s ? s : null));
  if (placeId && !privateLink && !jobId && (mode === "random" || mode === "smallest"))
    return { placeId, jobId, privateLink, mode };
  return { placeId, jobId, privateLink };
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const LOCALE = /^([a-z]{2}|[a-z]{2}-[a-z]{2})$/;

type Link = { placeId: number | null; jobId: string | null; priv: boolean };

/** Roblox link, scheme optional: Rust `parse_link` rules. `priv` = private-server or share link.
 * Null when it is not a Roblox game or share link. */
function parseLink(d: string, depth = 0): Link | null {
  let u: URL;
  try {
    u = new URL(/^[a-z]+:\/\//i.test(d) ? d : `https://${d}`);
  } catch {
    return null;
  }
  const h = u.hostname.toLowerCase();
  if (!/^https?:$/.test(u.protocol)) return null;
  if (h !== "ro.blox.com" && h !== "roblox.com" && !h.endsWith(".roblox.com")) return null;
  const q = (k: string) => u.searchParams.get(k) || null;
  // ro.blox.com redirect links carry the real target in af_web_dp (depth 1 only)
  const dp = q("af_web_dp");
  if (h === "ro.blox.com" && depth === 0 && dp) return parseLink(dp, 1);
  let segs = u.pathname.split("/").slice(1);
  if (LOCALE.test(segs[0]) && ["games", "share", "share-links"].includes(segs[1]))
    segs = segs.slice(1);
  if (segs[0] === "games") {
    const place = segs[1] === "start" ? q("placeId") : segs[1];
    const placeId = /^\d+$/.test(place ?? "") ? Number(place) : 0;
    if (!placeId) return null;
    const jobId = q("gameInstanceId");
    return {
      placeId,
      jobId: jobId && UUID.test(jobId) ? jobId : null,
      priv: !!q("privateServerLinkCode"),
    };
  }
  if (segs[0] === "share" || segs[0] === "share-links") {
    const ok = q("code") && ["Server", "ExperienceInvite"].includes(q("type") ?? "");
    return ok ? { placeId: null, jobId: null, priv: true } : null;
  }
  return null;
}
