import ipcSrc from "./ipc.ts?raw";
import buildRs from "../../src-tauri/build.rs?raw";
import libRs from "../../src-tauri/src/lib.rs?raw";
import caps from "../../src-tauri/capabilities/default.json";
import indicator from "../../src-tauri/capabilities/remote-indicator.json";

const ipcCmds = () => {
  const block = ipcSrc.match(/^type Commands = \{([\s\S]*?)^\};/m)?.[1] ?? "";
  return [...block.matchAll(/^ {2}([a-z_]+):/gm)].map((m) => m[1]!);
};
const allowOf = (c: string) => `allow-${c.replace(/_/g, "-")}`;

// Every command the UI can call must be granted, or Tauri rejects it with
// "Command X not allowed by ACL" at runtime only.
test("every Commands entry in ipc.ts is allowed in capabilities/default.json", () => {
  const cmds = ipcCmds();
  expect(cmds.length).toBeGreaterThan(50);
  const allowed = new Set(caps.permissions);
  const missing = cmds.filter((c) => !allowed.has(allowOf(c)));
  expect(missing).toEqual([]);
});

// The four lists a new command goes into (CONTRIBUTING): generate_handler! in lib.rs, the app
// manifest in build.rs, the main capability and ipc.ts. Both ways, so a stale entry fails too.
test("lib.rs, build.rs, default.json and ipc.ts list the same commands", () => {
  const handlers = (libRs.match(/generate_handler!\[([\s\S]*?)\]/)?.[1] ?? "")
    .split(",")
    .map((s) =>
      s
        .replace(/\/\/.*$/gm, "")
        .trim()
        .split("::")
        .pop()!,
    )
    .filter(Boolean)
    .sort();
  const manifest = [
    ...(buildRs.match(/\.commands\(&\[([\s\S]*?)\]/)?.[1] ?? "").matchAll(/"([a-z_]+)"/g),
  ]
    .map((m) => m[1]!)
    .sort();
  const allowed = caps.permissions
    .filter((p) => /^allow-[a-z-]+$/.test(p))
    .map((p) => p.slice(6).replace(/-/g, "_"))
    .sort();
  expect(handlers.length).toBeGreaterThan(50);
  expect(manifest).toEqual(handlers);
  expect(allowed).toEqual(handlers);
  expect([...ipcCmds()].sort()).toEqual(handlers);
});

test("the remote indicator window can only read and stop the session", () => {
  expect(indicator.permissions.filter((p) => p.startsWith("allow-")).sort()).toEqual([
    "allow-remote-status",
    "allow-remote-stop",
  ]);
});
