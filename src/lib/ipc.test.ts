import {
  call,
  codeText,
  errCode,
  errMessage,
  hasCode,
  mockBus,
  onEvent,
  VAULT_LOCKED_EVENT,
} from "./ipc";
import { t } from "./i18n";

test("errCode reads a bare or cmd-prefixed code from an Error or a string", () => {
  expect(errCode("WRONG_PASSWORD")).toBe("WRONG_PASSWORD");
  expect(errCode(new Error("vault_unlock: WRONG_PASSWORD"))).toBe("WRONG_PASSWORD");
  expect(errCode("Something went wrong")).toBe("");
  expect(errCode("wrong_password")).toBe("");
  expect(hasCode(new Error("x: VAULT_LOCKED"), "VAULT_LOCKED")).toBe(true);
  expect(hasCode("VAULT_LOCKED", "VAULT_WAS_LOCKED")).toBe(false);
});

test("codeText swaps known codes for catalog text, keeps the cmd prefix, passes the rest", () => {
  expect(codeText("vault_unlock: WRONG_PASSWORD")).toBe(`vault_unlock: ${t("vault.err.wrong")}`);
  expect(codeText("SOME_UNKNOWN")).toBe("SOME_UNKNOWN");
  // tool codes read as text on every surface (Outfits wear toast), not just the Tools sheet
  expect(errMessage("outfit_wear: SESSION_EXPIRED")).toBe(t("tools.sessionExpired"));
  expect(errMessage("outfit_wear: PIN_REQUIRED")).toBe(t("tools.pin.batch"));
  expect(codeText("plain text")).toBe("plain text");
});

test("errMessage drops the cmd prefix even when the code has no catalog text", () => {
  expect(errMessage("cmd_x: SOME_UNKNOWN")).toBe("SOME_UNKNOWN");
  expect(errMessage(new Error("Plain text"))).toBe("Plain text");
});

test("onEvent outside Tauri hears mockBus events until unsubscribed", () => {
  const cb = vi.fn();
  const off = onEvent<number>("test://ev", cb);
  mockBus.dispatchEvent(new CustomEvent("test://ev", { detail: 7 }));
  off();
  mockBus.dispatchEvent(new CustomEvent("test://ev", { detail: 8 }));
  expect(cb.mock.calls).toEqual([[7]]);
});

test("call() fires the vault-locked event for lock errors only", async () => {
  const { mockCall, resetVaultMock } = await import("./mock");
  resetVaultMock();
  await mockCall("vault_create", { mode: "password", password: "correct horse" });
  await mockCall("vault_lock");
  const fired = vi.fn();
  window.addEventListener(VAULT_LOCKED_EVENT, fired);
  try {
    await expect(
      call("vault_export_plain", { path: "/x", confirm: "export plaintext", password: null }),
    ).rejects.toThrow("vault_export_plain: VAULT_LOCKED");
    expect(fired).toHaveBeenCalledTimes(1);
    await expect(call("vault_create", { mode: "password", password: "short" })).rejects.toThrow(
      "vault_create: Password must be at least 8 characters.",
    );
    expect(fired).toHaveBeenCalledTimes(1);
  } finally {
    window.removeEventListener(VAULT_LOCKED_EVENT, fired);
  }
});

test("errMessage strips only the lowercase cmd prefix", () => {
  expect(errMessage(new Error("servers_stream: Sign in first."))).toBe("Sign in first.");
  expect(errMessage(new Error("Warning: Roblox is running"))).toBe("Warning: Roblox is running");
});
