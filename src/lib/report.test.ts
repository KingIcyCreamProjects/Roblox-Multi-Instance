import { useUi } from "@/app/store";
import { t } from "./i18n";
import { reportError } from "./report";

beforeEach(() => {
  useUi.setState({ lastError: null, moreErrors: [] });
  vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => vi.restoreAllMocks());

test("reportError logs the raw message and shows the user text in the banner", () => {
  reportError(new Error("vault_unlock: WRONG_PASSWORD"));
  expect(useUi.getState().lastError).toBe(t("vault.err.wrong"));
  expect(console.error).toHaveBeenCalledWith("vault_unlock: WRONG_PASSWORD");
});

test("reportError takes non-Error values as their string", () => {
  reportError("boom");
  expect(useUi.getState().lastError).toBe("boom");
  expect(console.error).toHaveBeenCalledWith("boom");
});

test("vault-locked errors are logged but never reach the banner", () => {
  reportError(new Error("VAULT_LOCKED"));
  reportError("VAULT_WAS_LOCKED");
  expect(useUi.getState().lastError).toBeNull();
  expect(console.error).toHaveBeenCalledTimes(2);
});
