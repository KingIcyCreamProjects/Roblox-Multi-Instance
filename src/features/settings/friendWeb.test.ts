// The friend's phone page (src-tauri/src/friend/web/app.js), run in jsdom against a fake server.
import appJs from "../../../src-tauri/src/friend/web/app.js?raw";
import indexHtml from "../../../src-tauri/src/friend/web/index.html?raw";

type Acc = {
  id: string;
  displayName: string;
  username: string;
  status: string;
  game: string | null;
  uptimeSec: number | null;
};
let acc: Acc;
let posts: { path: string; body: string; signal?: AbortSignal | null }[];
let postError: string | null;
/** set: /v1/accounts answers this instead of the list */
let listError: Response | null;
/** friend add: GET /v1/add says allowed; /v1/add/status answers addStatus; start may fail */
let addAllowed: boolean;
let addStatus: Record<string, unknown>;
let startError: string | null;
let calls: string[];

const json = (v: unknown) => new Response(JSON.stringify(v), { status: 200 });

function boot(status: string, uptimeSec: number | null, token = true) {
  vi.clearAllTimers();
  acc = { id: "a1", displayName: "Alt", username: "alt", status, game: null, uptimeSec };
  posts = [];
  document.body.innerHTML = indexHtml.slice(
    indexHtml.indexOf("<body>") + 6,
    indexHtml.indexOf("</body>"),
  );
  if (token) localStorage.setItem("rmiToken", "t");
  else localStorage.removeItem("rmiToken");
  postError = null;
  listError = null;
  addAllowed = false;
  addStatus = { status: "waiting" };
  startError = null;
  calls = [];
  vi.stubGlobal("fetch", async (path: string, opts: RequestInit = {}) => {
    calls.push(path);
    if (path === "/v1/add") return json({ allowed: addAllowed, max: 10, added: 0 });
    if (path === "/v1/add/status") return json(addStatus);
    if (path === "/v1/add/start" || path === "/v1/add/cancel") {
      posts.push({ path, body: "" });
      if (path === "/v1/add/start" && startError)
        return new Response(JSON.stringify({ error: { code: startError, message: "x" } }), {
          status: 409,
        });
      return json(path === "/v1/add/start" ? { code: "ABC123", expiresInSec: 300 } : {});
    }
    if (path === "/login") {
      posts.push({ path, body: String(opts.body ?? ""), signal: opts.signal });
      return json({ token: "t" });
    }
    if (opts.method === "POST") {
      posts.push({ path, body: String(opts.body ?? ""), signal: opts.signal });
      return postError
        ? new Response(JSON.stringify({ error: { code: postError } }), { status: 409 })
        : json({});
    }
    if (path === "/v1/accounts") return listError ? listError.clone() : json([acc]);
    // avatar. Not a real Response: reading a Blob body needs real I/O ticks on Node 22,
    // which the fake timers never run, so the image would not mount in time.
    return { ok: true, status: 200, blob: async () => new Blob(["png"]) } as unknown as Response;
  });
  new Function(appJs)();
}

const btn = () => document.querySelector<HTMLButtonElement>(".card .btn")!;
const sheet = () => document.getElementById("sheet") as HTMLDialogElement;
const wait = (ms: number) => vi.advanceTimersByTimeAsync(ms);

beforeAll(() => {
  URL.createObjectURL = () => "blob:avatar";
  HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
    this.open = true;
  };
  HTMLDialogElement.prototype.close = function (this: HTMLDialogElement, v?: string) {
    if (v !== undefined) this.returnValue = v;
    this.open = false;
    this.dispatchEvent(new Event("close"));
  };
});
beforeEach(() => vi.useFakeTimers());
afterEach(async () => {
  // stop this page instance: every poll loop parks on a fetch that never answers
  vi.stubGlobal("fetch", () => new Promise(() => {}));
  await wait(6000);
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

test("reconnect spins until Roblox is tracked, then a tap at the flip is ignored", async () => {
  boot("paused", null);
  await wait(10);
  expect(btn().textContent).toBe("Reconnect");
  btn().click();
  btn().click(); // double tap: one POST
  await wait(10);
  expect(posts.map((p) => p.path)).toEqual(["/v1/accounts/a1/reconnect"]);
  acc.status = "offline"; // launch started, no tracked client yet
  await wait(3000);
  expect(btn().disabled).toBe(true);
  expect(btn().textContent).toContain("Reconnecting");
  acc.status = "online"; // Roblox is up
  acc.uptimeSec = 3;
  await wait(1100);
  expect(btn().textContent).toBe("Disconnect");
  btn().click(); // aimed at the old Reconnect label
  expect(sheet().open).toBe(false);
  await wait(1600);
  btn().click();
  expect(sheet().open).toBe(true);
});

test("double tap on Disconnect keeps the confirm sheet open", async () => {
  boot("inGame", 60);
  await wait(10);
  btn().click();
  expect(sheet().open).toBe(true);
  sheet().click(); // second tap lands on the backdrop
  expect(sheet().open).toBe(true);
  await wait(500);
  sheet().click();
  expect(sheet().open).toBe(false);
  await wait(10);
  expect(posts).toEqual([]);
});

test("a hidden link field is not sent on Reconnect", async () => {
  boot("paused", null);
  await wait(10);
  const toggle = document.querySelector<HTMLButtonElement>(".card .link")!;
  const field = document.querySelector<HTMLInputElement>(".card input")!;
  toggle.click();
  field.value = "https://www.roblox.com/games/1";
  toggle.click(); // Use the same game
  btn().click();
  await wait(10);
  expect(posts[0]?.body).toBe("{}");
});

test("after log out and back in: no 'Updated' line on the login form, avatar comes back", async () => {
  boot("inGame", 60);
  await wait(10);
  expect(document.querySelector(".card img.av")).not.toBeNull();
  document.getElementById("logout")!.click();
  await wait(2000);
  expect(document.getElementById("updated")!.hidden).toBe(true);
  (document.getElementById("user") as HTMLInputElement).value = "me";
  (document.getElementById("pass") as HTMLInputElement).value = "pw";
  document.getElementById("login")!.dispatchEvent(new Event("submit", { cancelable: true }));
  await wait(10);
  expect(document.querySelector(".card img.av")).not.toBeNull();
});

test("a timed-out 'Still starting' note clears once Roblox is up", async () => {
  boot("paused", null);
  await wait(10);
  btn().click();
  acc.status = "online"; // the server shows a running RMI launch as online, no uptime yet
  await wait(31_000);
  const note = document.querySelector<HTMLElement>(".card .note")!;
  expect(note.textContent).toContain("Still starting");
  acc.status = "online";
  acc.uptimeSec = 3;
  await wait(5100);
  expect(note.hidden).toBe(true);
});

test("a launch that dies says Roblox did not start, not 'Still starting' forever", async () => {
  boot("paused", null);
  await wait(10);
  btn().click();
  acc.status = "online"; // launching
  await wait(1100);
  expect(btn().disabled).toBe(true);
  acc.status = "offline"; // the launch failed: no client, grace over
  await wait(1100);
  expect(document.querySelector(".card .note")!.textContent).toBe(
    "Roblox did not start. Try again.",
  );
  expect(btn().textContent).toBe("Reconnect");
  expect(btn().disabled).toBe(false);
});

test("an error note clears once the account's status moves on", async () => {
  boot("paused", null);
  postError = "slow_down";
  await wait(10);
  btn().click();
  await wait(10);
  const note = document.querySelector<HTMLElement>(".card .note")!;
  expect(note.textContent).toBe("Wait a few seconds and try again.");
  acc.status = "inGame"; // reconnected some other way
  acc.uptimeSec = 60;
  await wait(5100);
  expect(note.hidden).toBe(true);
});

test("the pending note is not rewritten each second (screen readers would repeat it)", async () => {
  boot("paused", null);
  await wait(10);
  btn().click();
  await wait(10);
  const node = document.querySelector(".card .note")!.firstChild;
  expect(node?.textContent).toContain("Starting Roblox");
  await wait(2100);
  expect(document.querySelector(".card .note")!.firstChild).toBe(node);
});

test("a gateway timeout keeps watching instead of saying try again", async () => {
  boot("paused", null);
  postError = "timeout";
  await wait(10);
  btn().click();
  await wait(10);
  expect(btn().disabled).toBe(true); // still pending, no red "try again"
  expect(document.querySelector(".card .note")!.classList.contains("err")).toBe(false);
  acc.status = "online";
  acc.uptimeSec = 3; // the launch went through after all
  await wait(1100);
  expect(btn().textContent).toBe("Disconnect");
});

test("requests carry a timeout; buttons name their account; Enter in the link field reconnects", async () => {
  boot("paused", null);
  await wait(10);
  expect(btn().getAttribute("aria-label")).toBe("Reconnect Alt");
  document.querySelector<HTMLButtonElement>(".card .link")!.click(); // show the field
  const field = document.querySelector<HTMLInputElement>(".card input")!;
  field.value = "https://www.roblox.com/games/1";
  field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
  await wait(10);
  expect(posts.map((p) => p.path)).toEqual(["/v1/accounts/a1/reconnect"]);
  expect(posts[0]!.signal).toBeInstanceOf(AbortSignal);
});

test("a bare 502 from the tunnel reads as the PC being unreachable", async () => {
  boot("paused", null);
  listError = new Response("Bad Gateway", { status: 502 });
  await wait(5100);
  expect(document.getElementById("banner")!.textContent).toBe("Can't reach the owner's computer.");
});

test("login trims the pasted password and leaves the form even if the first list call fails", async () => {
  boot("paused", null, false);
  listError = new Response(JSON.stringify({ error: { code: "locked" } }), { status: 503 });
  await wait(10);
  (document.getElementById("user") as HTMLInputElement).value = "me";
  (document.getElementById("pass") as HTMLInputElement).value = " pw ";
  document.getElementById("login")!.dispatchEvent(new Event("submit", { cancelable: true }));
  await wait(10);
  expect(JSON.parse(posts[0]!.body).password).toBe("pw");
  expect(document.getElementById("login")!.hidden).toBe(true);
  expect(document.getElementById("banner")!.textContent).toBe("The owner's RMI is locked.");
});

test("no_game shows the link field with a toggle that hides it again", async () => {
  boot("paused", null);
  postError = "no_game";
  await wait(10);
  btn().click();
  await wait(10);
  expect(document.querySelector<HTMLInputElement>(".card input")!.hidden).toBe(false);
  expect(document.querySelector(".card .link")!.textContent).toBe("Use the same game");
});

const $ = (id: string) => document.getElementById(id) as HTMLElement;
const addSheet = () => $("add-sheet") as HTMLDialogElement;
const count = (p: string) => calls.filter((c) => c === p).length;

test("no Add account button unless the owner allows it", async () => {
  boot("paused", null);
  await wait(10);
  expect(count("/v1/add")).toBe(1);
  expect($("add").hidden).toBe(true);
});

test("add account: code as text, waiting, linked, done, list refreshed, polling stops", async () => {
  boot("paused", null);
  addAllowed = true;
  await wait(10);
  expect($("add").hidden).toBe(false);
  $("add").click();
  $("add").click(); // double tap: one start
  await wait(10);
  expect(addSheet().open).toBe(true);
  expect(posts.map((p) => p.path)).toEqual(["/v1/add/start"]);
  expect($("add-code").textContent).toBe("ABC123");
  expect(document.querySelector("#add-sheet img")).toBeNull(); // no Roblox QR image
  expect($("add-msg").textContent).toBe("Waiting for you to confirm in Roblox");
  expect($("add-msg").getAttribute("aria-live")).toBe("polite");
  await wait(2100);
  expect(count("/v1/add/status")).toBe(1);
  addStatus = { status: "linked", accountName: "Kid" };
  await wait(2100);
  expect($("add-msg").textContent).toBe("Found Kid. Finishing…");
  const lists = count("/v1/accounts");
  addStatus = { status: "done", accountName: "Kid" };
  await wait(2100);
  expect($("add-msg").textContent).toBe("Added. It is in your list now.");
  expect($("add-done").hidden).toBe(false);
  expect($("add-cancel").hidden).toBe(true);
  expect(document.activeElement).toBe($("add-done"));
  expect(count("/v1/accounts")).toBeGreaterThan(lists);
  $("add-done").click(); // too soon after the swap: a tap aimed at Cancel is ignored
  expect(addSheet().open).toBe(true);
  const polls = count("/v1/add/status");
  await wait(6000);
  expect(count("/v1/add/status")).toBe(polls);
  $("add-done").click();
  expect(addSheet().open).toBe(false);
  expect(posts.map((p) => p.path)).not.toContain("/v1/add/cancel");
});

test("add account: Cancel tells the server and stops polling; an expired code offers a new one", async () => {
  boot("paused", null);
  addAllowed = true;
  await wait(10);
  $("add").click();
  await wait(2100);
  addStatus = { status: "expired" };
  await wait(2100);
  expect($("add-msg").textContent).toBe("This code expired.");
  expect($("add-new").hidden).toBe(false);
  await wait(1600);
  addStatus = { status: "waiting" };
  $("add-new").click();
  await wait(10);
  expect(posts.filter((p) => p.path === "/v1/add/start")).toHaveLength(2);
  await wait(1600);
  $("add-cancel").click();
  expect(addSheet().open).toBe(false);
  await wait(10);
  expect(posts.at(-1)?.path).toBe("/v1/add/cancel");
  const polls = count("/v1/add/status");
  await wait(6000);
  expect(count("/v1/add/status")).toBe(polls);
});

test("add account: full or off says why and offers no new code", async () => {
  boot("paused", null);
  addAllowed = true;
  startError = "add_full";
  await wait(10);
  $("add").click();
  await wait(10);
  expect($("add-msg").textContent).toBe(
    "You already added the most accounts allowed. Ask the owner to remove one.",
  );
  expect($("add-msg").classList.contains("err")).toBe(true);
  expect($("add-new").hidden).toBe(true);
  expect($("add-cancel").textContent).toBe("Close");
});

test("add account: no status polls while the tab is hidden", async () => {
  boot("paused", null);
  addAllowed = true;
  await wait(10);
  $("add").click();
  await wait(10);
  Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
  const polls = count("/v1/add/status");
  await wait(6100);
  expect(count("/v1/add/status")).toBe(polls);
  delete (document as { hidden?: boolean }).hidden; // back to jsdom's own (visible)
  await wait(2100);
  expect(count("/v1/add/status")).toBeGreaterThan(polls);
});

test("a second poll loop is not started by a tap during a poll", async () => {
  boot("paused", null);
  await wait(10);
  let polls = 0;
  const f = globalThis.fetch;
  vi.stubGlobal(
    "fetch",
    (p: string, o?: RequestInit) => (p === "/v1/accounts" && polls++, f(p, o)),
  );
  document.dispatchEvent(new Event("visibilitychange"));
  document.dispatchEvent(new Event("visibilitychange"));
  await wait(20_000);
  expect(polls).toBeLessThanOrEqual(6); // one loop at 5 s (two pokes + 4 ticks), not three
});
