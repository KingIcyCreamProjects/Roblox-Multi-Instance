// Dev/test-only Account tools backend (docs/contracts/tools.md). All data is synthetic.
// Quirks on purpose: the 2nd account is on a display-name cooldown and fails every batch,
// email changes need the PIN (1234) once, and the unlock lapses after 5 minutes.
import type { AccountView, BatchResult, BlockedUser, Privacy, PrivacyOption } from "./ipc";

type Ctx = { emit: (name: string, detail?: unknown) => void; displayNames: AccountView[] };

const STEP_MS = import.meta.env.MODE === "test" ? 2 : 350;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

let pinUntil = 0;
let blocked: BlockedUser[] = [];
let privacy = { follow: "Friends", privateServerInvite: "Friends" };
let email = "o****@example.com";
let batchSeq = 0;
/** Last URL passed to account_open_site (test hook). */
export let lastSite = "";
export function resetToolsMock() {
  pinUntil = 0;
  blocked = [
    { userId: 5001, username: "builder_bob", displayName: "Bob" },
    { userId: 5002, username: "spam_bot_77", displayName: "Totally Real" },
  ];
  privacy = { follow: "Friends", privateServerInvite: "Friends" };
  email = "o****@example.com";
}
resetToolsMock();

const OPTS: PrivacyOption[] = [
  { value: "AllUsers", label: "Everyone" },
  { value: "Friends", label: "Friends" },
  { value: "NoOne", label: "No one" },
];
const pinState = () => ({
  enabled: true,
  unlockedUntil: pinUntil > Date.now() ? new Date(pinUntil).toISOString() : null,
});
const validName = (u: unknown) => /^[A-Za-z0-9_]{3,20}$/.test(String(u));

/** Sequential per-account run; streams `tools://batch` like the real runner. */
async function batch(
  ctx: Ctx,
  a: Record<string, unknown>,
  run: (id: string) => string | null,
  failMsg: string,
  expireIdx = -1,
  allFail?: string,
): Promise<BatchResult[]> {
  const ids = a.accountIds as string[];
  const batchId = String(a.batchId ?? `batch-${++batchSeq}`);
  const out: BatchResult[] = [];
  for (const [i, id] of ids.entries()) {
    await sleep(STEP_MS);
    const fail = ctx.displayNames[1]?.id === id;
    const expired = ctx.displayNames[expireIdx]?.id === id;
    const result: BatchResult = allFail
      ? { accountId: id, ok: false, message: allFail }
      : expired
        ? { accountId: id, ok: false, message: "SESSION_EXPIRED" }
        : fail
          ? { accountId: id, ok: false, message: failMsg }
          : { accountId: id, ok: true, message: run(id) };
    out.push(result);
    ctx.emit("tools://batch", { batchId, result, done: i + 1, total: ids.length });
  }
  return out;
}

/** undefined: not a tools command. null: void result. */
export function toolsCall(cmd: string, a: Record<string, unknown>, ctx: Ctx) {
  switch (cmd) {
    case "tool_pin_state":
      return pinState();
    case "tool_pin_unlock":
      if (a.pin !== "1234") throw "Wrong PIN. Check the four digits and try again.";
      pinUntil = Date.now() + 5 * 60_000;
      return pinState();
    case "tool_display_name":
      return batch(
        ctx,
        a,
        (id) => {
          const acc = ctx.displayNames.find((x) => x.id === id);
          if (acc) acc.displayName = String(a.name);
          return null;
        },
        "Display name changed too recently. Roblox allows one change every 7 days, so try again later.",
      );
    case "tool_password":
      if (a.current === "wrong") throw "Current password is incorrect. Check it and try again.";
      return null;
    case "tool_email_get":
      return { masked: email, verified: true };
    case "tool_email_set":
      if (pinUntil < Date.now()) throw "PIN_REQUIRED";
      if (a.password === "wrong") throw "Password is incorrect. Check it and try again.";
      email = String(a.email).replace(/^(.).*(@.*)$/, "$1****$2");
      return null;
    case "tool_privacy_get": {
      const p: Privacy = {
        ...privacy,
        followOptions: OPTS,
        privateServerInviteOptions: OPTS,
      };
      return p;
    }
    case "tool_privacy_set":
      return batch(
        ctx,
        a,
        () => {
          if (a.follow) privacy.follow = String(a.follow);
          if (a.privateServerInvite) privacy.privateServerInvite = String(a.privateServerInvite);
          return null;
        },
        "Roblox did not accept the change. Try again in a few minutes.",
      );
    case "tool_signout_others":
      return null;
    case "tool_friend_request":
      if (!validName(a.username)) throw "No Roblox user has that name.";
      // like Rust: a batch reports PIN_REQUIRED per row, never as a thrown error
      if (a.username === "pin_needed" && pinUntil < Date.now())
        return batch(ctx, a, () => null, "", -1, "PIN_REQUIRED");
      return batch(
        ctx,
        a,
        () => "Request sent",
        "Friend limit reached or request already pending.",
        3,
      );
    case "tool_block":
      if (!validName(a.username)) throw "No Roblox user has that name.";
      return batch(
        ctx,
        a,
        () => {
          const u = String(a.username);
          blocked = blocked.filter((b) => b.username.toLowerCase() !== u.toLowerCase());
          if (a.block) blocked.push({ userId: 5100 + blocked.length, username: u, displayName: u });
          return a.block ? "Blocked" : "Unblocked";
        },
        "Roblox did not accept the change. Try again in a few minutes.",
      );
    case "tool_blocked_list":
      return structuredClone(blocked);
    case "tool_unblock_all": {
      const count = blocked.length;
      blocked = [];
      return { count, failed: 0, truncated: false };
    }
    case "tool_robux":
      return { robux: 12_480 };
    case "tool_info": {
      const acc = ctx.displayNames.find((x) => x.id === a.accountId);
      if (!acc) throw "Account not found.";
      return {
        userId: acc.userId ?? 0,
        username: acc.username,
        displayName: acc.displayName,
        created: "2019-03-14T00:00:00.000Z",
        ageDays: 2_597,
        profileUrl: `https://www.roblox.com/users/${acc.userId}/profile`,
      };
    }
    case "tool_export_summary":
      return { path: `C:/Users/you/Documents/${a.accountId}-summary.json` };
    case "account_open_site":
      lastSite = a.url as string;
      return null;
  }
  return undefined;
}
