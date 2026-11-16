import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useUi } from "@/app/store";
import { usePlayBar } from "@/features/playbar/store";
import { call } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { resetAccountsMock } from "@/lib/mock";
import { REAL_GAMES } from "@/lib/mockGames";
import { resetDiscoverMock } from "@/lib/mockDiscover";
import { Home } from "./Home";

const wrap = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tooltip.Provider>
        <Home />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );

afterEach(() => {
  resetAccountsMock(0);
  resetDiscoverMock(false);
});

test("hero shows the most recent game, with strips below", async () => {
  resetAccountsMock(6);
  resetDiscoverMock(true);
  wrap();
  expect(
    await screen.findByRole("heading", { level: 1, name: "Murder Mystery 2" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: t("home.jump") })).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: t("home.online") })).toBeInTheDocument();
});

test("no recents or favourites shows the quiet prompt", async () => {
  resetAccountsMock(3);
  wrap();
  expect(await screen.findByRole("button", { name: t("home.empty.cta") })).toBeInTheDocument();
});

test("the prompt opens Discover", async () => {
  resetAccountsMock(3);
  wrap();
  await userEvent.click(await screen.findByRole("button", { name: t("home.empty.cta") }));
  expect(useUi.getState().route).toBe("discover");
  useUi.setState({ route: "accounts" });
});

test("no recents: the first favourite fills the hero", async () => {
  resetAccountsMock(3);
  resetDiscoverMock(true);
  await call("recents_clear");
  wrap();
  expect(
    await screen.findByRole("heading", { level: 1, name: REAL_GAMES[2].name }),
  ).toBeInTheDocument();
  expect(screen.getByText(t("home.favorite"))).toBeInTheDocument();
});

test("hero Play needs an account: disabled with the hint until one is picked", async () => {
  localStorage.removeItem("rmi.lastLaunchSet");
  useUi.setState({ sel: { ids: [], anchor: null, cursor: null } });
  resetAccountsMock(6);
  resetDiscoverMock(true);
  wrap();
  await screen.findByRole("heading", { level: 1 });
  expect(screen.getByRole("button", { name: t("playbar.play") })).toBeDisabled();
  expect(screen.getByText(t("home.playNeedsAccount"))).toBeInTheDocument();
});

test("hero Play uses the current selection before the last launched set", async () => {
  localStorage.setItem("rmi.lastLaunchSet", JSON.stringify(["acc-3", "acc-4"]));
  useUi.setState({ sel: { ids: ["acc-1"], anchor: "acc-1", cursor: "acc-1" } });
  resetAccountsMock(6);
  resetDiscoverMock(true);
  wrap();
  await screen.findByRole("heading", { level: 1 });
  expect(screen.getByRole("button", { name: t("playbar.play") })).toBeEnabled();
  localStorage.removeItem("rmi.lastLaunchSet");
  useUi.setState({ sel: { ids: [], anchor: null, cursor: null } });
});

test("Online now lists only online and in-game accounts", async () => {
  resetAccountsMock(6);
  wrap();
  const list = await call("accounts_list");
  const online = list.accounts.filter(
    (a) => a.presence?.state === "online" || a.presence?.state === "in_game",
  );
  expect(online.length).toBeGreaterThan(0);
  const region = await screen.findByRole("region", { name: t("home.online") });
  expect(within(region).getAllByRole("button")).toHaveLength(online.length);
});

test("Jump back in: a recent becomes the Play bar destination", async () => {
  resetAccountsMock(6);
  resetDiscoverMock(true);
  usePlayBar.setState({ destination: "" });
  wrap();
  const [first] = await call("recents_list");
  await userEvent.click(
    await screen.findByRole("button", { name: t("home.jumpItem", { name: first.name }) }),
  );
  expect(usePlayBar.getState().destination).toBe(String(first.placeId));
});
