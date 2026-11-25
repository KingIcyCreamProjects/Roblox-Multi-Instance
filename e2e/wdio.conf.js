// E2E smoke on Windows: WebdriverIO -> tauri-driver -> msedgedriver -> debug build.
// Run with `corepack pnpm e2e` (builds `tauri build --debug --no-bundle` first).
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const drivers = path.join(here, ".drivers");
const tauriDriver = path.join(os.homedir(), ".cargo", "bin", "tauri-driver.exe");
const app = path.join(here, "..", "src-tauri", "target", "debug", "roblox-multi-instance.exe");

// Isolated instance: own data dir (debug-only RMI_DATA_DIR: also skips single-instance and
// window-state). Set once in the launcher; workers inherit it, and so do tauri-driver ->
// msedgedriver -> app. msedgedriver gives the WebView2 its own temp profile.
// Always a fresh dir (a pre-set RMI_DATA_DIR is replaced, never used or deleted).
const made = process.env.WDIO_WORKER_ID ? null : mkdtempSync(path.join(os.tmpdir(), "rmi-e2e-"));
if (made) process.env.RMI_DATA_DIR = made;

/** msedgedriver must match the installed WebView2 runtime version. */
async function edgeDriver() {
  const key =
    "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
  const out = execFileSync("reg", ["query", key, "/v", "pv"], { encoding: "utf8" });
  const version = out.match(/pv\s+REG_SZ\s+(\S+)/)[1];
  const dir = path.join(drivers, version);
  const exe = path.join(dir, "msedgedriver.exe");
  if (existsSync(exe)) return exe;
  mkdirSync(dir, { recursive: true });
  const res = await fetch(`https://msedgedriver.microsoft.com/${version}/edgedriver_win64.zip`);
  if (!res.ok) throw new Error(`msedgedriver ${version} download failed: ${res.status}`);
  const zip = path.join(dir, "driver.zip");
  writeFileSync(zip, Buffer.from(await res.arrayBuffer()));
  // System32 bsdtar reads zip; a GNU tar earlier on PATH (Git Bash) does not
  execFileSync(path.join(process.env.SystemRoot, "System32", "tar.exe"), ["-xf", zip, "-C", dir]);
  return exe;
}

let driver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4445,
  specs: ["./specs/**/*.e2e.js"],
  maxInstances: 1,
  capabilities: [{ "tauri:options": { application: app } }],
  reporters: ["spec"],
  framework: "mocha",
  mochaOpts: { ui: "bdd", timeout: 120_000 },
  logLevel: "warn",

  onPrepare: async () => {
    if (!existsSync(tauriDriver)) {
      const r = spawnSync("cargo", ["install", "tauri-driver", "--locked"], { stdio: "inherit" });
      if (r.status !== 0) throw new Error("tauri-driver install failed");
    }
    process.env.RMI_EDGEDRIVER = await edgeDriver(); // read by the worker below
  },

  beforeSession: () => {
    driver = spawn(
      tauriDriver,
      ["--port", "4445", "--native-port", "4446", "--native-driver", process.env.RMI_EDGEDRIVER],
      { stdio: [null, process.stdout, process.stderr] },
    );
  },

  // kill() ends only tauri-driver on Windows; /T also ends the msedgedriver and app it started
  afterSession: () => {
    if (driver?.exitCode === null)
      spawnSync("taskkill", ["/PID", String(driver.pid), "/T", "/F"], { stdio: "ignore" });
  },

  // keep the data dir of a failed run for its logs
  onComplete: (code) => {
    if (code === 0 && made) rmSync(made, { recursive: true, force: true, maxRetries: 5 });
  },
};
