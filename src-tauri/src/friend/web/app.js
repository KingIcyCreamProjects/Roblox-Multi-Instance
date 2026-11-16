"use strict";
// RMI Remote. Same-origin only. Server strings go through textContent, never innerHTML.
const KEY = "rmiToken";
const $ = (id) => document.getElementById(id);
const STATUS = { inGame: "In game", online: "Online", offline: "Offline", paused: "Paused" }; // paused: the detail line says "Disconnected by you"
const FAST_MS = 1000; // poll rate while an action is settling
const SLOW_MS = 5000;
const SETTLE_MS = 30000; // give up waiting for the status flip after this
const HOLD_MS = 1500; // ignore taps this long after the button flips: a tap aimed at the old label must not run the new one
const SHEET_MS = 400; // ignore taps this long around the confirm sheet: the second tap of a double tap
const NO_REACH = "Can't reach the owner's computer.";
const cards = new Map(); // id -> { li, refs, acc, pend, late, shown, hold }
const avatars = new Map(); // id -> { url, t }

let token = null;
let busy = false;
let again = false;
let timer = 0;
let lastOk = 0;
let quiet = 0; // no card taps before this time (confirm sheet just closed)

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function saveToken(t) {
  try { localStorage.setItem(KEY, t); } catch { /* private mode: token lives for this page only */ }
  token = t;
}

function dropToken() {
  token = null;
  try { localStorage.removeItem(KEY); } catch { /* nothing stored */ }
}

// AbortSignal.timeout is Safari 16+: older browsers just wait as before
const timeout = () => AbortSignal.timeout?.(15000);

async function api(path, opts = {}) {
  let res;
  try {
    res = await fetch(path, {
      ...opts,
      headers: { ...opts.headers, Authorization: "Bearer " + token },
      // a stalled request (cellular handoff, resumed home-screen app) must not freeze polling
      signal: timeout(),
    });
  } catch {
    throw { code: "network", message: NO_REACH };
  }
  if (res.ok) return res;
  let e = {};
  try { e = (await res.json()).error || {}; } catch { /* non-JSON error */ }
  // a bare 502/503/504 is the tunnel's answer: RMI or the friend server is not running
  if (!e.code && res.status >= 502) throw { code: "network", status: res.status, message: NO_REACH };
  throw { code: e.code || "http_" + res.status, status: res.status, message: e.message || "Something went wrong" };
}

// Login error text for an HTTP status (0: no response).
function loginError(status) {
  if (status === 0 || status >= 502) return NO_REACH; // 502-504: the tunnel, not RMI
  if (status === 401) return "Wrong username or password";
  if (status === 429) return "Too many tries. Wait a minute.";
  if (status === 403) return "Open the link the owner sent you.";
  return "Something went wrong";
}

// Logged out: forget the token and the shown accounts, show the login form.
function showLogin(msg) {
  dropToken();
  for (const c of cards.values()) c.li.remove();
  cards.clear();
  lastOk = 0;
  $("login-msg").textContent = msg || "";
  $("login-msg").hidden = !msg;
  $("login").hidden = false;
  $("logout").hidden = true;
  $("list").hidden = true;
  $("empty").hidden = true;
  $("banner").hidden = true;
  $("updated").hidden = true;
  $("add").hidden = true;
  addAt = 0;
  if ($("add-sheet").open) $("add-sheet").close();
  loading();
}

function banner(text) {
  // same text: leave the node alone, or role=alert re-announces it on every failed poll
  if ($("banner").textContent !== (text || "")) $("banner").textContent = text || "";
  $("banner").hidden = !text;
  $("list").classList.toggle("stale", !!text);
  loading();
}

// Logged in, no list yet and no error: say so instead of an empty page.
function loading() {
  $("loading").hidden = !(token && !lastOk && $("banner").hidden);
}

function uptime(s) {
  if (s == null) return "";
  const m = Math.floor(s / 60);
  if (m < 1) return "under 1m";
  const h = Math.floor(m / 60);
  if (h < 24) return h ? `${h}h ${m % 60}m` : `${m}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

function updatedText() {
  if (!lastOk) return;
  const s = Math.round((Date.now() - lastOk) / 1000);
  $("updated").textContent = s < 5 ? "Updated just now" : s < 60 ? `Updated ${s}s ago` : `Updated ${Math.floor(s / 60)}m ago`;
  $("updated").hidden = false;
}

function mountAvatar(c, url) {
  if (c.refs.av.tagName === "IMG") return;
  const img = el("img", "av");
  img.alt = "";
  img.src = url;
  c.refs.av.replaceWith(img);
  c.refs.av = img;
}

async function loadAvatar(c) {
  const id = c.acc.id;
  const hit = avatars.get(id);
  if (hit?.url) return mountAvatar(c, hit.url); // cached: a re-created card still needs the image
  if (hit && Date.now() - hit.t < 60000) return;
  avatars.set(id, { url: null, t: Date.now() });
  try {
    const url = URL.createObjectURL(await (await api(`/v1/accounts/${encodeURIComponent(id)}/avatar`)).blob());
    avatars.set(id, { url, t: Date.now() });
    mountAvatar(c, url);
  } catch { /* 404 or offline: keep initials */ }
}

function makeCard(acc) {
  const li = el("li", "card");
  const head = el("div", "head");
  const av = el("div", "av");
  const who = el("div", "who");
  const name = el("div", "name");
  const sub = el("div", "sub");
  who.append(name, sub);
  const pill = el("span", "pill");
  head.append(av, who, pill);
  const detail = el("div", "detail");
  const game = el("span");
  const up = el("span");
  detail.append(game, up);
  const acts = el("div", "acts");
  const note = el("p", "note");
  note.setAttribute("role", "status");
  note.hidden = true;
  li.append(head, detail, acts, note);

  const field = el("input");
  field.type = "url";
  field.placeholder = "Game link or place ID";
  field.autocomplete = "off";
  field.autocapitalize = "off";
  field.spellcheck = false;
  field.maxLength = 1000;
  field.hidden = true;
  field.setAttribute("aria-label", "Game link, private server link or place ID");
  field.onkeydown = (e) => { if (e.key === "Enter") { e.preventDefault(); run(c); } }; // Android "Go"
  const toggle = el("button", "link", "Use a different game");
  toggle.type = "button";
  toggle.onclick = () => { field.hidden = !field.hidden; toggle.textContent = field.hidden ? "Use a different game" : "Use the same game"; if (!field.hidden) field.focus(); };
  const act = el("button", "btn");
  act.type = "button";
  act.onclick = () => run(c);
  acts.append(field, act, toggle); // the link first, then the button that uses it

  const c = { li, acc, pend: null, refs: { av, name, sub, pill, game, up, act, field, toggle, note } };
  return c;
}

// inGame/online: RMI has a client for it, offer Disconnect. offline/paused (or unknown): Reconnect.
const isLive = (acc) => acc.status === "inGame" || acc.status === "online";

function note(c, text, err, spin) {
  const n = c.refs.note;
  // unchanged: keep the node, or role=status re-announces it on every 1 s repaint
  if (n.textContent === (text || "") && n.classList.contains("err") === !!err && !!n.firstElementChild === !!(spin && text)) return;
  n.replaceChildren();
  if (spin && text) n.append(el("span", "spin"));
  if (text) n.append(document.createTextNode(text));
  n.hidden = !text;
  n.classList.toggle("err", !!err);
}

// Has the server caught up with an action? Reconnect waits for a tracked client
// (uptimeSec non-null), Disconnect for it to be gone.
const settled = (kind, acc) => kind === "disconnect" ? !isLive(acc) : isLive(acc) && acc.uptimeSec != null;

// A red note stays until the account's status moves on (or, for network/locked, the next good
// poll; for brief ones like slow_down, about 8 s).
function errNote(c, text, net, brief) {
  c.errS = c.acc.status;
  c.errNet = !!net;
  c.errUntil = brief ? Date.now() + 8000 : 0;
  note(c, text, true);
}
function clearErr(c) {
  c.errS = null;
  c.errNet = false;
  c.errUntil = 0;
  note(c, "");
}

const NO_START = "Roblox did not start. Try again.";

// Drop a pending action once the server shows its result, or after SETTLE_MS.
function settle(c) {
  const p = c.pend;
  if (!p) return;
  if (!p.posting && settled(p.kind, c.acc)) { c.pend = null; note(c, ""); return; }
  if (!p.posting && p.kind === "reconnect") {
    // the server shows "online" while the launch runs: live then gone, or never live, means it failed
    if (isLive(c.acc)) p.seen = true;
    else if (p.seen || Date.now() - p.t0 > SETTLE_MS) { c.pend = null; errNote(c, NO_START); return; }
  }
  if (Date.now() - p.t0 > SETTLE_MS) {
    c.pend = null;
    c.late = p.kind;
    note(c, p.kind === "disconnect" ? "Still closing. This card updates when it is done." : "Still starting. This card updates when Roblox is ready.");
  }
}

function paint(c, acc) {
  c.acc = acc;
  if (c.errS && (acc.status !== c.errS || (c.errUntil && Date.now() > c.errUntil))) clearErr(c);
  settle(c);
  const r = c.refs;
  const label = acc.displayName || acc.username;
  r.name.textContent = label;
  r.sub.textContent = "@" + acc.username;
  // in a game on another computer: not "Offline / Not running". The button stays Reconnect
  // (nothing here to disconnect); pressing it starts the account on this computer.
  const away = !!acc.elsewhere;
  r.pill.textContent = away ? STATUS.inGame : STATUS[acc.status] || acc.status;
  r.pill.dataset.s = away ? "inGame" : acc.status;
  // in game before the place name resolves: never "In game" next to "Not in a game"
  r.game.textContent = away ? (acc.game ? acc.game + " · " : "") + "On another computer" : acc.game || (acc.status === "inGame" ? "In a game" : acc.status === "online" ? "Not in a game" : acc.status === "paused" ? "Disconnected by you" : "Not running");
  r.up.textContent = isLive(acc) && acc.uptimeSec != null ? "Up " + uptime(acc.uptimeSec) : "";
  if (!avatars.get(acc.id)?.url && r.av.tagName !== "IMG") r.av.textContent = (label[0] || "?").toUpperCase();
  const live = isLive(acc);
  const p = c.pend;
  if (!p && c.late && settled(c.late, acc)) { c.late = null; note(c, ""); }
  // "Still starting", then the client went away: the launch failed
  else if (!p && c.late === "reconnect" && !live) { c.late = null; errNote(c, NO_START); }
  let text;
  if (p) {
    text = p.kind === "disconnect" ? "Disconnecting…" : "Reconnecting…";
    if (p.kind === "reconnect") note(c, "Starting Roblox. This can take a minute.");
  } else {
    text = live ? "Disconnect" : "Reconnect";
    if (c.shown && c.shown !== text) c.hold = Date.now() + HOLD_MS;
    c.shown = text;
  }
  // rebuild only on a change: a new spinner every 1 s repaint would restart its turn
  if (c.btnText !== text) {
    c.btnText = text;
    r.act.replaceChildren();
    if (p) r.act.append(el("span", "spin"));
    r.act.append(document.createTextNode(text));
  }
  // several cards: screen readers need to know whose button this is
  r.act.setAttribute("aria-label", `${text} ${label}`);
  r.act.disabled = !!p;
  r.act.classList.toggle("accent", !live && !p);
  r.act.classList.toggle("danger", live && !p);
  c.li.classList.toggle("busy", !!p);
  r.toggle.hidden = live || !!p;
  if (live || p) { r.field.hidden = true; r.toggle.textContent = "Use a different game"; }
  loadAvatar(c);
}

// Glass confirm sheet in place of confirm().
function confirmSheet(label) {
  const d = $("sheet");
  $("sheet-title").textContent = `Disconnect ${label}?`;
  return new Promise((res) => {
    d.addEventListener("close", () => { quiet = Date.now() + SHEET_MS; res(d.returnValue === "yes"); }, { once: true });
    d.returnValue = "no";
    quiet = Date.now() + SHEET_MS;
    d.showModal();
  });
}
// Backdrop tap cancels, but not the second tap of the double tap that opened the sheet.
$("sheet").addEventListener("click", (ev) => { if (ev.target === ev.currentTarget && Date.now() >= quiet) ev.currentTarget.close("no"); });

async function run(c) {
  const { acc, refs } = c;
  if (c.pend || Date.now() < Math.max(quiet, c.hold || 0)) return;
  const kind = isLive(acc) ? "disconnect" : "reconnect";
  if (kind === "disconnect" && !(await confirmSheet(acc.displayName || acc.username))) return;
  if (c.pend) return;
  const link = refs.field.hidden ? "" : refs.field.value.trim();
  c.pend = { kind, t0: Date.now(), posting: true };
  c.late = null;
  clearErr(c);
  paint(c, c.acc); // spinner + dim now, before the network answers
  poke();
  try {
    if (kind === "disconnect") await api(`/v1/accounts/${encodeURIComponent(acc.id)}/disconnect`, { method: "POST" });
    else {
      await api(`/v1/accounts/${encodeURIComponent(acc.id)}/reconnect`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(link ? { link } : {}),
      });
    }
    refs.field.value = "";
    if (c.pend) { c.pend.posting = false; c.pend.t0 = Date.now(); }
    paint(c, c.acc);
    refresh();
  } catch (e) {
    if (e.code === "timeout") {
      // the server gave up waiting, but the launch or close keeps going: watch for the result
      c.pend = { kind, t0: Date.now(), posting: false };
      paint(c, c.acc);
      note(c, "The owner's computer is slow. Checking…");
      poke();
      return;
    }
    c.pend = null;
    paint(c, c.acc);
    if (e.code === "no_game" || e.code === "bad_link") {
      refs.field.hidden = false;
      refs.toggle.textContent = "Use the same game";
      refs.field.focus();
      errNote(c, e.code === "no_game" ? "This account has no game saved. Paste a game link below." : "Paste a Roblox game link or place ID.");
    } else if (e.code === "unauthorized") {
      fail(e);
    } else if (e.code === "slow_down") {
      errNote(c, "Wait a few seconds and try again.", false, true);
    } else if (e.code === "busy") {
      errNote(c, "Already reconnecting.", false, true);
    } else if (e.code === "forbidden_host") {
      fail(e);
    } else if (e.code === "locked") {
      fail(e);
      errNote(c, "The owner's RMI is locked. Ask them to unlock it.", true);
    } else if (e.code === "network") {
      fail(e);
      errNote(c, "Not sent. Try again when it's back.", true);
    } else {
      errNote(c, e.message);
    }
  }
}

function fail(e) {
  if (e.code === "unauthorized") {
    showLogin("Logged out. Log in again.");
  } else if (e.code === "forbidden_host") banner("Open the link the owner sent you.");
  else if (e.code === "locked") banner("The owner's RMI is locked.");
  else if (e.code === "network") banner(NO_REACH);
  else banner(e.code ? e.message : "Something went wrong"); // no code: a bad body (Safari's SyntaxError text is cryptic)
}

function render(list) {
  const ul = $("list");
  const ids = new Set(list.map((a) => a.id));
  for (const [id, c] of cards) if (!ids.has(id)) { c.li.remove(); cards.delete(id); }
  list.forEach((acc, i) => {
    let c = cards.get(acc.id);
    if (!c) { c = makeCard(acc); cards.set(acc.id, c); }
    paint(c, acc);
    if (ul.children[i] !== c.li) ul.insertBefore(c.li, ul.children[i] || null);
  });
  $("empty").hidden = list.length > 0;
}

async function refresh() {
  if (!token) return;
  if (busy) { again = true; return; } // an action's result shows right after the poll in flight
  busy = true;
  try {
    const list = await (await api("/v1/accounts")).json();
    if (!token) return; // dropped meanwhile
    $("login").hidden = true;
    $("logout").hidden = false;
    $("list").hidden = false;
    banner("");
    render(list);
    for (const c of cards.values()) if (c.errNet) clearErr(c); // reachable and unlocked again
    lastOk = Date.now();
    loading();
    updatedText();
    if (Date.now() - addAt > ADD_INFO_MS) addInfo();
  } catch (e) {
    fail(e);
  } finally {
    busy = false;
    if (again) { again = false; refresh(); }
  }
}

// One poll loop: 1 s while any card waits on the server, else 5 s.
async function tick() {
  if (!document.hidden) await refresh();
  for (const c of cards.values()) if (c.pend) { settle(c); paint(c, c.acc); }
  clearTimeout(timer); // a poke() during the await started a second tick: keep one loop
  timer = setTimeout(tick, [...cards.values()].some((c) => c.pend) ? FAST_MS : SLOW_MS);
}
function poke() { clearTimeout(timer); tick(); }

$("login").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const btn = $("login-btn");
  if (btn.disabled) return;
  // empty fields would still cost one of the server's login tries
  const empty = !$("user").value.trim() ? $("user") : !$("pass").value.trim() ? $("pass") : null;
  if (empty) {
    $("login-msg").textContent = "Enter the username and password.";
    $("login-msg").hidden = false;
    empty.focus();
    return;
  }
  btn.disabled = true;
  $("login-msg").hidden = true; // the old error is not about this try
  btn.replaceChildren(el("span", "spin"), document.createTextNode("Logging in…"));
  let status = 0;
  let t = null;
  try {
    const res = await fetch("/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      // a password copied out of a chat often carries a stray space; generated ones never have one
      body: JSON.stringify({ username: $("user").value, password: $("pass").value.trim() }),
      signal: timeout(),
    });
    status = res.status;
    if (res.ok) t = (await res.json()).token;
  } catch { /* network: status 0 */ }
  btn.disabled = false;
  btn.textContent = "Log in";
  if (typeof t !== "string" || !t) {
    $("login-msg").textContent = loginError(status);
    $("login-msg").hidden = false;
    return;
  }
  $("pass").value = "";
  $("login-msg").hidden = true;
  // logged in even if the first list call fails (locked, offline): that shows as a banner
  $("login").hidden = true;
  $("logout").hidden = false;
  saveToken(t);
  loading();
  poke();
});

$("logout").addEventListener("click", () => showLogin(""));

// Add account: the friend signs in to his own Roblox account with Quick Log In. The code is
// shown as text only; the sign-in itself happens on the owner's computer.
const ADD_INFO_MS = 30000; // re-ask whether adding is allowed at most this often
const ADD_POLL_MS = 2000;
const ADD_ERR = {
  add_full: "You already added the most accounts allowed. Ask the owner to remove one.",
  add_off: "The owner turned off adding accounts.",
  locked: "The owner's RMI is locked. Ask them to unlock it.",
  slow_down: "Too many codes. Wait a few minutes and try again.",
  network: NO_REACH,
};
let addAt = 0; // last /v1/add check
let addLive = false; // a code is out: keep polling while the sheet is open
let addTimer = 0;
let addHold = 0; // sheet buttons ignore taps until then (they just swapped)
let addBusy = false;
let addState = "";

async function addInfo() {
  addAt = Date.now();
  let ok = false;
  try { ok = (await (await api("/v1/add")).json()).allowed === true; } catch { /* locked, offline or an older RMI: no button */ }
  $("add").hidden = !ok || !token;
}

// Status line under the code; unchanged text keeps its node (aria-live would repeat it).
function addSay(text, kind) {
  const n = $("add-msg");
  if (n.textContent === text && n.classList.contains("err") === (kind === "err") && !!n.firstElementChild === (kind === "spin")) return;
  n.replaceChildren();
  if (kind === "spin") n.append(el("span", "spin"));
  n.append(document.createTextNode(text));
  n.classList.toggle("err", kind === "err");
}

// "pending": code out; "done": added; "over": expired/failed, maybe a new code; "stop": no retry.
function addButtons(state) {
  if (state !== addState) addHold = Date.now() + HOLD_MS;
  addState = state;
  $("add-copy").disabled = !$("add-code").textContent;
  $("add-copy").hidden = state !== "pending"; // a used or dead code is not worth copying
  $("add-done").hidden = state !== "done";
  $("add-new").hidden = state !== "over";
  $("add-cancel").hidden = state === "done";
  $("add-cancel").textContent = state === "pending" ? "Cancel" : "Close";
}

const showCode = (code) => { $("add-code").textContent = code || ""; };

function addOver(text, retry) {
  addLive = false;
  clearTimeout(addTimer);
  addSay(text, "err");
  addButtons(retry ? "over" : "stop");
  (retry ? $("add-new") : $("add-cancel")).focus();
}

function addError(e) {
  if (e.code === "unauthorized" || e.code === "forbidden_host") { $("add-sheet").close(); fail(e); return; }
  if (e.code === "add_off") $("add").hidden = true;
  addOver(ADD_ERR[e.code] || e.message || "Something went wrong", !(e.code === "add_full" || e.code === "add_off"));
}

async function addStart() {
  addLive = true;
  showCode("");
  addSay("Getting a code…", "spin");
  addButtons("pending");
  try {
    const r = await (await api("/v1/add/start", { method: "POST" })).json();
    if (!$("add-sheet").open) return;
    showCode(r.code);
    addButtons("pending");
    addSay("Waiting for you to confirm in Roblox", "spin");
    addLater();
  } catch (e) {
    if ($("add-sheet").open) addError(e);
  }
}

function addLater() {
  clearTimeout(addTimer);
  if (addLive && $("add-sheet").open) addTimer = setTimeout(addPoll, ADD_POLL_MS);
}

async function addPoll() {
  if (!addLive || !$("add-sheet").open || addBusy) return;
  if (document.hidden) return addLater(); // no requests from a background tab
  addBusy = true;
  let s;
  try {
    s = await (await api("/v1/add/status")).json();
  } catch (e) {
    if (e.code === "network" || e.code === "timeout") { addSay(NO_REACH + " Still trying…", "err"); addLater(); }
    else if ($("add-sheet").open) addError(e);
    return;
  } finally {
    addBusy = false;
  }
  if (!addLive || !$("add-sheet").open) return;
  if (s.status === "waiting") addSay("Waiting for you to confirm in Roblox", "spin");
  else if (s.status === "linked") addSay(s.accountName ? `Found ${s.accountName}. Finishing…` : "Found your account. Finishing…", "spin");
  else if (s.status === "done") {
    addLive = false;
    addSay("Added. It is in your list now.", "");
    addButtons("done");
    $("add-done").focus();
    poke(); // the new account shows in the list
    return;
  } else if (s.status === "expired") return addOver("This code expired.", true);
  else if (s.status === "cancelled") return addOver("Cancelled in Roblox.", true);
  else return addOver(s.message || "Could not add the account.", true);
  addLater();
}

$("add").addEventListener("click", () => {
  if ($("add-sheet").open || Date.now() < quiet) return;
  addState = "";
  $("add-sheet").showModal();
  quiet = Date.now() + SHEET_MS;
  $("add-title").focus();
  addStart();
});
$("add-new").addEventListener("click", () => { if (Date.now() >= addHold) addStart(); });
$("add-done").addEventListener("click", () => { if (Date.now() >= addHold) $("add-sheet").close(); });
$("add-cancel").addEventListener("click", () => { if (Date.now() >= addHold) $("add-sheet").close(); });
$("add-copy").addEventListener("click", async () => {
  const b = $("add-copy");
  try { await navigator.clipboard.writeText($("add-code").textContent); b.textContent = "Copied"; } catch { b.textContent = "Copy did not work. Type the code."; }
  setTimeout(() => { b.textContent = "Copy"; }, 2000);
});
// Closing (Cancel, Close, Done or Escape) while a code is out cancels it.
$("add-sheet").addEventListener("close", () => {
  quiet = Date.now() + SHEET_MS;
  clearTimeout(addTimer);
  if (addLive && token) api("/v1/add/cancel", { method: "POST" }).catch(() => {});
  addLive = false;
  if (!$("add").hidden) $("add").focus();
});

try { token = localStorage.getItem(KEY); } catch { token = null; }
if (!token) showLogin("");
else loading();
setInterval(updatedText, 1000);
document.addEventListener("visibilitychange", () => { if (!document.hidden && token) poke(); });
tick();
