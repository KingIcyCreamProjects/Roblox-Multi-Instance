import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import * as Tooltip from "@radix-ui/react-tooltip";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ErrorBoundary } from "./ErrorBoundary";
import { Shell } from "./Shell";
import { useUi } from "./store";
import { queryClient } from "@/lib/queryClient";
import { t } from "@/lib/i18n";

afterEach(() => {
  useUi.setState({ lastError: null });
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
  vi.resetModules();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

test("error boundary shows message and reload", () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  const Boom = () => {
    throw new Error("kaboom");
  };
  render(
    <ErrorBoundary>
      <Boom />
    </ErrorBoundary>,
  );
  expect(screen.getByText(t("error.title"))).toBeInTheDocument();
  expect(screen.getByText("kaboom")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: t("error.reload") })).toBeInTheDocument();
});

test("shell banner shows lastError and dismisses", async () => {
  useUi.setState({ lastError: "Could not save settings: disk full" });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tooltip.Provider>
        <Shell />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  expect(screen.getByText("Could not save settings: disk full")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: t("error.dismiss") }));
  expect(screen.queryByText(/disk full/)).toBeNull();
});

test("query errors reach the banner state via the shared client", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  await queryClient
    .fetchQuery({ queryKey: ["x"], queryFn: () => Promise.reject(new Error("nope")) })
    .catch(() => {});
  await waitFor(() => expect(useUi.getState().lastError).toBe("nope"));
});

test("ipc: production outside Tauri throws, no mock", async () => {
  vi.stubEnv("DEV", false);
  vi.stubEnv("MODE", "production");
  const { call } = await import("@/lib/ipc");
  await expect(call("app_info")).rejects.toThrow("app_info: not running inside Tauri");
});

test("ipc: tauri invoke rejection is wrapped with command name", async () => {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  vi.doMock("@tauri-apps/api/core", () => ({ invoke: () => Promise.reject("boom") }));
  const { call } = await import("@/lib/ipc");
  await expect(call("settings_get")).rejects.toThrow("settings_get: boom");
});

test("ipc: error prefix is stripped for display through the real call()", async () => {
  const { call, errMessage, hasCode } = await import("@/lib/ipc");
  const { resetVaultMock } = await import("@/lib/mock");
  const { vaultError } = await import("@/features/vault/useVault");
  resetVaultMock();
  await call("vault_create", { mode: "password", password: "correct horse" });
  await call("vault_lock");
  const e = await call("vault_unlock", { password: "nope nope", remember: false }).catch(
    (x: unknown) => x,
  );
  expect((e as Error).message).toBe("vault_unlock: WRONG_PASSWORD");
  expect(hasCode(e, "WRONG_PASSWORD")).toBe(true);
  expect(errMessage(e)).toBe("Wrong password.");
  expect(vaultError(e)).toBe(t("vault.err.wrong"));
});

test("silent queries and mutations with their own onError stay out of the banner", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  useUi.setState({ lastError: null, moreErrors: [] });
  const boom = () => Promise.reject(new Error("quiet"));
  await queryClient
    .fetchQuery({ queryKey: ["silent-probe"], queryFn: boom, meta: { silent: true } })
    .catch(() => {});
  await queryClient
    .getMutationCache()
    .build(queryClient, { mutationFn: boom, onError: () => {} })
    .execute(undefined)
    .catch(() => {});
  expect(useUi.getState().lastError).toBeNull();
  await queryClient
    .getMutationCache()
    .build(queryClient, { mutationFn: boom })
    .execute(undefined)
    .catch(() => {});
  expect(useUi.getState().lastError).toBe("quiet");
});
