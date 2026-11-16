import type { AccountView, AccountsList } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import {
  ago,
  buildRows,
  cookiesIn,
  destText,
  emptySel,
  initials,
  label,
  matches,
  move,
  pendingIn,
  select,
  selectGroup,
} from "./model";

const acc = (id: string, p: Partial<AccountView> = {}): AccountView =>
  ({
    id,
    userId: 1,
    username: `${id}_user`,
    displayName: id.toUpperCase(),
    alias: null,
    note: null,
    group: "Main",
    color: null,
    pinned: false,
    order: 0,
    session: "ok",
    presence: null,
    ...p,
  }) as AccountView;

const order = ["a", "b", "c", "d", "e"];

test("click replaces, ctrl toggles, shift spans from anchor", () => {
  let s = select(emptySel, order, "b", "replace");
  expect(s.ids).toEqual(["b"]);
  s = select(s, order, "d", "range");
  expect(s.ids).toEqual(["b", "c", "d"]);
  s = select(s, order, "a", "range"); // anchor stays b
  expect(s.ids).toEqual(["a", "b"]);
  s = select(s, order, "e", "toggle");
  expect(s.ids).toEqual(["a", "b", "e"]);
  s = select(s, order, "a", "toggle");
  expect(s.ids).toEqual(["b", "e"]);
  expect(s.anchor).toBe("a");
});

test("arrows move, shift-arrows extend, ctrl-arrows move only the cursor", () => {
  let s = move(emptySel, order, 1, "replace");
  expect(s).toMatchObject({ ids: ["a"], cursor: "a" });
  s = move(s, order, 1, "range");
  s = move(s, order, 1, "range");
  expect(s.ids).toEqual(["a", "b", "c"]);
  s = move(s, order, 1, "cursor");
  expect(s).toMatchObject({ ids: ["a", "b", "c"], cursor: "d" });
  s = move(s, order, 99, "replace");
  expect(s.ids).toEqual(["e"]);
  expect(move(s, order, -Infinity, "replace").ids).toEqual(["a"]);
});

test("select group adds the rest, then clears when all are in", () => {
  let s = select(emptySel, order, "a", "replace");
  s = selectGroup(s, ["c", "d"]);
  expect(s.ids).toEqual(["a", "c", "d"]);
  s = selectGroup(s, ["c", "d"]);
  expect(s.ids).toEqual(["a"]);
});

test("search covers alias, username, display name, note and group", () => {
  const a = acc("x", { alias: "Farm Bot", note: "trade later", group: "Grinders" });
  for (const q of ["farm", "x_us", "X", "TRADE", "grind", "  "]) expect(matches(a, q)).toBe(true);
  expect(matches(a, "nope")).toBe(false);
});

test("rows: group order, pinned first, collapsed and search", () => {
  const list: AccountsList = {
    groups: ["Alts", "Main"],
    groupTargets: {},
    accounts: [
      acc("m1", { order: 0 }),
      acc("m2", { order: 1, pinned: true }),
      acc("a1", { group: "Alts", alias: "zed" }),
      acc("o1", { group: "Other" }),
    ],
  };
  const keys = (q: string, c = new Set<string>()) => buildRows(list, q, c).map((r) => r.key);
  expect(keys("")).toEqual(["group:Alts", "a1", "group:Main", "m2", "m1", "group:Other", "o1"]);
  expect(keys("", new Set(["Main"]))).toEqual([
    "group:Alts",
    "a1",
    "group:Main",
    "group:Other",
    "o1",
  ]);
  expect(keys("zed")).toEqual(["group:Alts", "a1"]);
  expect(keys("zed", new Set(["Alts"]))).toEqual(["group:Alts", "a1"]); // search opens groups
});

test("cookie-shaped strings are pulled out of dropped text", () => {
  const text = 'foo .ROBLOSECURITY="_|WARNING:-DO-NOT-SHARE.|_ABC123"; bar\n_|WARNING:x|_DEF';
  expect(cookiesIn(text)).toEqual(["_|WARNING:-DO-NOT-SHARE.|_ABC123", "_|WARNING:x|_DEF"]);
  expect(cookiesIn("hello")).toEqual([]);
});

test("username:password lines are found when no cookies are present", () => {
  expect(pendingIn("hello world\n alt_one:pw1 \nbad line\nx:y")).toEqual(["alt_one:pw1"]);
});

test("shift-click onto an anchor that left the list selects just the clicked row", () => {
  const s = { ids: ["a"], anchor: "zz", cursor: "a" };
  expect(select(s, order, "c", "range")).toEqual({ ids: ["c"], anchor: "c", cursor: "c" });
});

test("label: alias, then display name, then username", () => {
  expect(label(acc("x", { alias: "Al", displayName: "Dee" }))).toBe("Al");
  expect(label(acc("x", { displayName: "Dee" }))).toBe("Dee");
  expect(label(acc("x", { displayName: undefined }))).toBe("x_user");
});

test("initials: two words or humps, one letter, ? when nothing is left", () => {
  expect(initials(acc("x", { alias: "Farm Bot" }))).toBe("FB");
  expect(initials(acc("x", { alias: "blueBird" }))).toBe("BB");
  expect(initials(acc("x", { alias: "alice" }))).toBe("A");
  expect(initials(acc("x", { alias: "!!" }))).toBe("?");
});

test("destText: game name or Place N, then private server / JobId / mode", () => {
  expect(destText(null)).toBeNull();
  const place = { placeId: 1818, jobId: null, privateLink: null };
  expect(destText(place)).toBe("Place 1818");
  expect(destText(place, "Obby")).toBe("Obby");
  expect(destText({ placeId: null, jobId: null, privateLink: null })).toBe("Place ?");
  expect(destText({ placeId: null, jobId: null, privateLink: "https://x" })).toBe("Private server");
  expect(destText({ placeId: 1, jobId: null, privateLink: "https://x" }, "Obby")).toBe(
    "Obby · Private server",
  );
  expect(destText({ placeId: 1, jobId: "0b9a7c2e-1f3d", privateLink: null }, "Obby")).toBe(
    "Obby · server 0b9a7c2e",
  );
  expect(destText({ ...place, mode: "random" }, "Obby")).toBe(`Obby · ${t("mode.random")}`);
  expect(destText({ ...place, mode: "smallest" })).toBe("Place 1818 · Smallest");
});

test("ago: under a minute is now, then past or future units", () => {
  const now = Date.parse("2026-11-08T12:00:00Z");
  expect(ago("2026-11-08T11:59:30Z", now)).toBe("now");
  expect(ago("2026-11-08T10:00:00Z", now)).toBe("2 hours ago");
  expect(ago("2026-11-08T12:05:00Z", now)).toBe("in 5 minutes");
});

test("ago: days, months and years", () => {
  const now = Date.parse("2026-11-08T12:00:00Z");
  expect(ago("2026-11-06T12:00:00Z", now)).toBe("2 days ago");
  expect(ago("2026-08-08T12:00:00Z", now)).toBe("3 months ago");
  expect(ago("2024-11-08T12:00:00Z", now)).toBe("2 years ago");
});
