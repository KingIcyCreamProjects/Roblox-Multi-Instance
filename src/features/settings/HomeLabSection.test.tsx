import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { openExternal } from "@/lib/events";
import { t } from "@/lib/i18n";
import { mockCaps, mockHomeLab, mockVault, resetVaultMock } from "@/lib/mock";
import { HomeLabSection } from "./HomeLabSection";

vi.mock("@/lib/events", async (orig) => ({
  ...(await orig<typeof import("@/lib/events")>()),
  openExternal: vi.fn(),
}));

beforeEach(() => {
  resetVaultMock();
  vi.mocked(openExternal).mockClear();
});

const setup = () =>
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <HomeLabSection />
    </QueryClientProvider>,
  );

test("shows the latest version and downloads it", async () => {
  const user = userEvent.setup();
  setup();
  expect(screen.getByText(t("hl.checking"))).toBeInTheDocument();
  expect(await screen.findByText(t("hl.latest", { v: "1.4.0" }))).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("hl.download") }));
  expect(await screen.findByRole("status")).toHaveTextContent("HomeLabSetup.exe");
  expect(mockVault.lastArgs).toHaveProperty("homelab_download");
  await user.click(screen.getByRole("button", { name: t("hl.github") }));
  expect(openExternal).toHaveBeenCalledWith(expect.stringContaining("/HomeLab/releases/tag/"));
});

test("not Windows: plain Download, never opens or runs", async () => {
  const user = userEvent.setup();
  mockCaps.os = "macos";
  setup();
  const btn = await screen.findByRole("button", { name: t("hl.downloadOnly") });
  await waitFor(() => expect(screen.queryByRole("button", { name: t("hl.download") })).toBeNull());
  await user.click(btn);
  expect(await screen.findByRole("status")).toHaveTextContent("HomeLabSetup.exe");
  expect(openExternal).not.toHaveBeenCalled();
});

test("no build for this computer: says so, with the GitHub link", async () => {
  const user = userEvent.setup();
  mockHomeLab.mode = "nobuild";
  setup();
  expect(await screen.findByText(t("hl.noBuild"))).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: t("fs.retry") })).toBeNull();
  expect(screen.queryByRole("button", { name: t("hl.downloadOnly") })).toBeNull();
  await user.click(screen.getByRole("button", { name: t("hl.github") }));
  expect(openExternal).toHaveBeenCalledWith(
    "https://github.com/KingIcyCreamProjects/HomeLab/releases",
  );
});

test("a release page elsewhere is never opened", async () => {
  const user = userEvent.setup();
  mockHomeLab.page = "https://evil.example/KingIcyCreamProjects/HomeLab/";
  setup();
  await screen.findByText(t("hl.latest", { v: "1.4.0" }));
  await user.click(screen.getByRole("button", { name: t("hl.github") }));
  expect(openExternal).toHaveBeenCalledWith(
    "https://github.com/KingIcyCreamProjects/HomeLab/releases",
  );
});

test("offline or not released yet: calm state with the GitHub link and a retry", async () => {
  const user = userEvent.setup();
  mockHomeLab.mode = "offline";
  setup();
  expect(await screen.findByText(t("hl.offline"))).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: t("hl.download") })).toBeNull();
  await user.click(screen.getByRole("button", { name: t("hl.github") }));
  expect(openExternal).toHaveBeenCalledWith(
    "https://github.com/KingIcyCreamProjects/HomeLab/releases",
  );
  mockHomeLab.mode = "ok";
  await user.click(screen.getByRole("button", { name: t("fs.retry") }));
  expect(await screen.findByText(t("hl.latest", { v: "1.4.0" }))).toBeInTheDocument();
});
