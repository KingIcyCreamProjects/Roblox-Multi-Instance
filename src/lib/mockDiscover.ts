// Dev/test-only Discover backend (docs/contracts/discover.md). Games are real public Roblox
// games (mockGames.ts); servers, accounts and outfits are synthetic.
import type {
  Favorite,
  GameTile,
  MissingAsset,
  Outfit,
  Recent,
  Region,
  ServerRow,
  UniversePlace,
} from "./ipc";
import { REAL_GAMES } from "./mockGames";

type Emit = (name: string, detail?: unknown) => void;

const isTest = import.meta.env.MODE === "test";
const PAGE_MS = isTest ? 5 : 90;

/** Deterministic PRNG so the same place always yields the same servers. */
function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const hash = (s: string) => [...s].reduce((h, c) => (Math.imul(h, 31) + c.charCodeAt(0)) | 0, 7);

/** Synthetic placeholder art: a two-hue gradient with soft shapes, as an SVG data URI. */
export function art(seed: string, w = 768, h = 432) {
  const r = rng(hash(seed));
  const h1 = Math.floor(r() * 360);
  const h2 = (h1 + 40 + Math.floor(r() * 100)) % 360;
  const shapes = Array.from({ length: 5 }, () => {
    const cx = Math.floor(r() * w);
    const cy = Math.floor(r() * h);
    const rad = Math.floor((0.12 + r() * 0.3) * h);
    return `<circle cx="${cx}" cy="${cy}" r="${rad}" fill="hsl(${(h1 + r() * 60) | 0} 70% ${40 + r() * 25}%)" opacity="${(0.25 + r() * 0.4).toFixed(2)}"/>`;
  }).join("");
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${h1} 65% 46%)"/><stop offset="1" stop-color="hsl(${h2} 60% 22%)"/></linearGradient></defs><rect width="${w}" height="${h}" fill="url(#g)"/>${shapes}</svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

// ── games (real public Roblox games, see mockGames.ts) ───────────
const CATALOG: GameTile[] = REAL_GAMES.map((g, i) => ({
  ...g,
  likeRatio: 0.62 + rng(i + 11)() * 0.35,
}));
const byPlace = (placeId: number) => CATALOG.find((g) => g.placeId === placeId);
const tileFor = (placeId: number): GameTile =>
  byPlace(placeId) ?? {
    ...CATALOG[Math.abs(placeId) % CATALOG.length],
    placeId,
    universeId: placeId + 1,
    name: `Place ${placeId}`,
  };

// ── servers ──────────────────────────────────────────────────────
const PAGES = 100; // 100 pages × 100 rows = 10k rows for the virtualized table
const uuid = (r: () => number) =>
  "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const v = Math.floor(r() * 16);
    return (c === "x" ? v : (v & 0x3) | 0x8).toString(16);
  });

function serverPage(placeId: number, page: number): ServerRow[] {
  const r = rng(placeId * 131 + page);
  const max = [8, 12, 20, 30, 50][placeId % 5];
  return Array.from({ length: 100 }, () => {
    const roll = r();
    return {
      jobId: uuid(r),
      playing: roll < 0.25 ? max : roll < 0.3 ? 0 : Math.max(1, Math.floor(r() * max)),
      maxPlayers: max,
      ping: r() < 0.08 ? null : Math.round(25 + r() ** 2 * 260),
      fps: r() < 0.08 ? null : Math.round((48 + r() * 12) * 10) / 10,
      kind: "public",
      name: null,
    };
  });
}

const CITIES: Exclude<Region, null>[] = [
  { label: "Frankfurt, DE", countryCode: "DE" },
  { label: "Amsterdam, NL", countryCode: "NL" },
  { label: "London, GB", countryCode: "GB" },
  { label: "Ashburn, US", countryCode: "US" },
  { label: "Los Angeles, US", countryCode: "US" },
  { label: "Singapore, SG", countryCode: "SG" },
  { label: "Tokyo, JP", countryCode: "JP" },
  { label: "Sydney, AU", countryCode: "AU" },
  { label: "Paris, FR", countryCode: "FR" },
];

const timers = new Map<string, ReturnType<typeof setInterval>>();
let seq = 0;

function stream(tick: (n: number, id: string) => boolean) {
  const id = `stream-${++seq}`;
  let n = 0;
  const h = setInterval(() => {
    n += 1;
    if (tick(n, id)) {
      clearInterval(h);
      timers.delete(id);
    }
  }, PAGE_MS);
  timers.set(id, h);
  return { streamId: id };
}

// ── library state ────────────────────────────────────────────────
let favorites: Favorite[] = [];
let recents: Recent[] = [];
const iso = (minsAgo: number) => new Date(Date.now() - minsAgo * 60_000).toISOString();

export function resetDiscoverMock(withLibrary = !isTest) {
  timers.forEach(clearInterval);
  timers.clear();
  favorites = withLibrary
    ? [2, 5, 9, 13].map((g, i) => ({
        id: `fav-${i}`,
        placeId: CATALOG[g].placeId,
        name: CATALOG[g].name,
        privateLink: i === 2 ? "https://www.roblox.com/share?code=synthetic&type=Server" : null,
        thumbnail: CATALOG[g].thumbnail,
        order: i,
      }))
    : [];
  recents = withLibrary
    ? [7, 0, 2, 15, 12, 4, 10, 11].map((g, i) => ({
        placeId: CATALOG[g].placeId,
        name: CATALOG[g].name,
        thumbnail: CATALOG[g].thumbnail,
        lastPlayedAt: iso(20 + i * i * 95),
      }))
    : [];
}
resetDiscoverMock();

/** Launch mock records a played place (newest first, deduped). */
export function pushRecent(r: Recent) {
  recents = [r, ...recents.filter((x) => x.placeId !== r.placeId)];
}

const OUTFIT_NAMES = [
  "Weekend fit",
  "Knight",
  "Racer",
  "Cozy hoodie",
  "Formal",
  "Neon runner",
  "Pirate",
  "Astronaut",
  "Beach day",
  "Winter coat",
];

/** Returns undefined when `cmd` is not a Discover command. */
export function discoverCall(cmd: string, a: Record<string, unknown>, emit: Emit): unknown {
  switch (cmd) {
    case "servers_stream": {
      const placeId = a.placeId as number;
      if (a.kind === "private") {
        if (!a.accountId) throw "Private servers need a selected account.";
        const r = rng(placeId);
        const rows: ServerRow[] = ["Squad night", "Trading only", "Practice"].map((name) => ({
          ...serverPage(placeId, 999)[0],
          jobId: uuid(r),
          kind: "private",
          name,
        }));
        return stream((n, streamId) => {
          emit("servers://page", {
            streamId,
            rows,
            page: n,
            done: true,
            error: null,
            stopped: false,
          });
          return true;
        });
      }
      return stream((n, streamId) => {
        if (n === 30 && !isTest)
          emit("roblox://ratelimited", { host: "games.roblox.com", retryInMs: 2500 });
        const done = n >= PAGES;
        emit("servers://page", {
          streamId,
          rows: serverPage(placeId, n),
          page: n,
          done,
          error: null,
          stopped: false,
        });
        return done;
      });
    }
    case "servers_stop": {
      const h = timers.get(a.streamId as string);
      if (h) clearInterval(h);
      timers.delete(a.streamId as string);
      return null;
    }
    case "servers_find_player": {
      const placeId = a.placeId as number;
      const name = String(a.username).toLowerCase();
      const at = name === "nobody" ? Infinity : 3 + (Math.abs(hash(name)) % 12);
      return stream((n, streamId) => {
        const hit = n >= at;
        const done = hit || n >= PAGES;
        emit("servers://find", {
          streamId,
          pagesScanned: n,
          found: hit ? { jobId: serverPage(placeId, n)[7].jobId } : null,
          done,
          error: null,
          stopped: false,
        });
        return done;
      });
    }
    case "servers_regions":
      return Object.fromEntries(
        (a.jobIds as string[]).map((j) => [j, CITIES[Math.abs(hash(j)) % CITIES.length]]),
      );
    case "servers_reserve_join":
      return { batchId: `batch-${++seq}` };
    case "games_discover":
      return structuredClone(CATALOG.slice(0, 24));
    case "games_search": {
      const q = String(a.query).trim().toLowerCase();
      const hits = CATALOG.filter((g) => g.name.toLowerCase().includes(q));
      const from = Number(a.pageToken ?? 0);
      const next = from + 12 < hits.length ? String(from + 12) : null;
      return { games: structuredClone(hits.slice(from, from + 12)), nextPageToken: next };
    }
    case "game_details":
      return (a.placeIds as number[]).map(tileFor);
    case "universe_of":
      return { universeId: tileFor(a.placeId as number).universeId };
    case "universe_places": {
      const g = CATALOG.find((x) => x.universeId === a.universeId) ?? CATALOG[0];
      const subs = ["Lobby", "Arena", "Trading Hub", "Winter Event"];
      const out: UniversePlace[] = [{ placeId: g.placeId, name: g.name, isRoot: true }];
      subs.forEach((s, i) =>
        out.push({ placeId: g.placeId + 10 + i, name: `${g.name}: ${s}`, isRoot: false }),
      );
      return out;
    }
    case "favorites_list":
      return structuredClone([...favorites].sort((x, y) => x.order - y.order));
    case "favorites_add": {
      const link = (a.privateLink as string | null) ?? null;
      const dup = favorites.find((f) => f.placeId === a.placeId && f.privateLink === link);
      if (dup) return structuredClone(dup);
      const f: Favorite = {
        id: `fav-${Date.now()}-${++seq}`,
        placeId: a.placeId as number,
        name: a.name as string,
        privateLink: link,
        thumbnail: tileFor(a.placeId as number).thumbnail,
        order: favorites.length,
      };
      favorites.push(f);
      return structuredClone(f);
    }
    case "favorites_update": {
      const f = favorites.find((x) => x.id === a.id);
      if (!f) throw "Favorite not found.";
      Object.assign(f, a.patch);
      return structuredClone(f);
    }
    case "favorites_remove":
      favorites = favorites.filter((x) => x.id !== a.id);
      return null;
    case "favorites_reorder":
      (a.ids as string[]).forEach((id, i) => {
        const f = favorites.find((x) => x.id === id);
        if (f) f.order = i;
      });
      return null;
    case "recents_list":
      return structuredClone(recents);
    case "recents_clear":
      recents = [];
      return null;
    case "outfits_list": {
      const u = String(a.username).trim();
      if (!/^[A-Za-z0-9_]{3,20}$/.test(u)) throw "No Roblox user has that name.";
      const r = rng(hash(u.toLowerCase()));
      const n = 3 + Math.floor(r() * 8);
      const outfits: Outfit[] = Array.from({ length: n }, (_, i) => ({
        id: 9_000_000 + (Math.abs(hash(u)) % 100_000) + i,
        name: OUTFIT_NAMES[(i + Math.floor(r() * 10)) % OUTFIT_NAMES.length],
        thumbnail: art(`${u}-outfit-${i}`, 320, 320),
      }));
      return { userId: Math.abs(hash(u)) % 9_000_000, outfits };
    }
    case "outfit_wear": {
      const id = a.outfitId as number;
      const missing: MissingAsset[] =
        id % 3 === 0
          ? [
              {
                assetId: 48_000_000 + id,
                name: "Classic Fedora",
                url: `https://www.roblox.com/catalog/${48_000_000 + id}`,
              },
              {
                assetId: 49_000_000 + id,
                name: "Sparkle Wings",
                url: `https://www.roblox.com/catalog/${49_000_000 + id}`,
              },
            ]
          : [];
      return { missing };
    }
  }
  return undefined;
}
