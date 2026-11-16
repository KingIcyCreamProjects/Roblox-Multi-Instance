import ipcSrc from "./ipc.ts?raw";
import caps from "../../src-tauri/capabilities/default.json";

// Every command the UI can call must be granted, or Tauri rejects it with
// "Command X not allowed by ACL" at runtime only.
test("every Commands entry in ipc.ts is allowed in capabilities/default.json", () => {
  const block = ipcSrc.match(/^type Commands = \{([\s\S]*?)^\};/m)?.[1] ?? "";
  const cmds = [...block.matchAll(/^ {2}([a-z_]+):/gm)].map((m) => m[1]);
  expect(cmds.length).toBeGreaterThan(50);
  const allowed = new Set(caps.permissions);
  const missing = cmds.filter((c) => !allowed.has(`allow-${c.replace(/_/g, "-")}`));
  expect(missing).toEqual([]);
});
