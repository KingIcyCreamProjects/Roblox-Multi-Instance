import { $, $$, expect } from "@wdio/globals";
import { randomBytes } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

describe("first run", () => {
  it("creates a password vault and lands on the empty accounts screen", async () => {
    await expect($("h1=Set up your vault")).toBeDisplayed({ wait: 30_000 });

    const pw = randomBytes(18).toString("base64url");
    const [first, confirm] = await $$('input[type="password"]');
    await first.setValue(pw);
    await confirm.setValue(pw);
    await $("button=Create vault").click();

    // Argon2 key derivation can take a few seconds in a debug build.
    await $("nav").waitForExist({ timeout: 60_000 });
    // a fresh data dir opens the first-run tour; its scrim covers the tabs until closed
    const close = $('button[aria-label="Close tutorial"]');
    await close.waitForDisplayed({ timeout: 30_000 });
    await close.click();
    await close.waitForExist({ reverse: true, timeout: 15_000 });
    await $("nav").$("button*=Accounts").click();
    await expect($("h1=Your accounts live here")).toBeDisplayed();
    await expect($("button*=Sign in with Roblox")).toBeDisplayed();

    const logs = path.join(process.env.RMI_DATA_DIR, "logs");
    const files = readdirSync(logs);
    expect(files).toContain("rmi.log");
    // only the hit count, never the log text (a hit means a cookie leaked)
    const leaked = files.filter((f) =>
      readFileSync(path.join(logs, f), "utf8").includes("_|WARNING"),
    );
    expect(leaked).toEqual([]);
  });
});
