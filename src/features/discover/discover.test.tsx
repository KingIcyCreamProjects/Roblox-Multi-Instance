import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useToasts } from "@/components/Toasts";
import { usePlayBar } from "@/features/playbar/store";
import { mockBus, type Favorite } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { resetDiscoverMock } from "@/lib/mockDiscover";
import { Favorites, reorderFavs, undoOrder } from "./Favorites";
import { Games } from "./Games";
import { Outfits } from "./Outfits";
import { Universe } from "./Universe";
import {
  copyText,
  FAV_KEY,
  fmtPct,
  joinLink,
  useDebounced,
  useDiscover,
  useRateLimited,
  sendToBar,
  Thumb,
  useRoving,
} from "./shared";

test("search debounces 350 ms", () => {
  vi.useFakeTimers();
  const { result, rerender } = renderHook(({ v }) => useDebounced(v, 350), {
    initialProps: { v: "" },
  });
  rerender({ v: "s" });
  act(() => void vi.advanceTimersByTime(200));
  rerender({ v: "sky" });
  act(() => void vi.advanceTimersByTime(300));
  expect(result.current).toBe("");
  act(() => void vi.advanceTimersByTime(50));
  expect(result.current).toBe("sky");
  vi.useRealTimers();
});

test("favourites reorder moves one and renumbers order", () => {
  const f = (id: string, order: number) =>
    ({ id, order, placeId: order, name: id, privateLink: null, thumbnail: null }) as Favorite;
  const list = [f("a", 0), f("b", 1), f("c", 2)];
  const next = reorderFavs(list, "c", "a");
  expect(next.map((x) => x.id)).toEqual(["c", "a", "b"]);
  expect(next.map((x) => x.order)).toEqual([0, 1, 2]);
  expect(reorderFavs(list, "a", "a")).toBe(list);
});

test("Set as destination writes the Play bar store", async () => {
  resetDiscoverMock(false);
  usePlayBar.setState({ destination: "" });
  const user = userEvent.setup();
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tooltip.Provider>
        <Games />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  const [first] = await screen.findAllByRole("button", { name: t("games.setDest") });
  await user.click(first);
  expect(usePlayBar.getState().destination).toBe("74260430392611");
});

test("undo of a remove keeps reorders made since, restored id back at its old spot", () => {
  const qc = new QueryClient();
  const fav = (id: string) => ({ id }) as Favorite;
  // removed "b" (was index 1), then the user moved "d" to the front
  qc.setQueryData(FAV_KEY, ["d", "a", "c"].map(fav));
  expect(undoOrder(qc, 1, "b2")).toEqual(["d", "b2", "a", "c"]);
  expect(undoOrder(qc, 9, "b2")).toEqual(["d", "a", "c", "b2"]);
});

test("Servers tab takes the Play bar place only when its input is empty", () => {
  usePlayBar.setState({ destination: "https://www.roblox.com/games/555/some-game" });
  useDiscover.setState({ tab: "games", place: "" });
  useDiscover.getState().setTab("servers");
  expect(useDiscover.getState()).toMatchObject({ tab: "servers", place: "555" });
  useDiscover.setState({ tab: "games", place: "9" });
  useDiscover.getState().setTab("servers");
  expect(useDiscover.getState().place).toBe("9");
});

test("join link and percent formats", () => {
  expect(joinLink(74260430392611, "job-1")).toBe(
    "https://www.roblox.com/games/start?placeId=74260430392611&gameInstanceId=job-1",
  );
  expect(fmtPct(0.666)).toBe("67%");
  expect(fmtPct(1)).toBe("100%");
});

test("rate limit holds for the longest announced wait, then clears", () => {
  vi.useFakeTimers();
  try {
    rateLimitHolds();
  } finally {
    vi.useRealTimers();
  }
});

function rateLimitHolds() {
  const { result } = renderHook(() => useRateLimited());
  const announce = (retryInMs: number) =>
    act(
      () =>
        void mockBus.dispatchEvent(
          new CustomEvent("roblox://ratelimited", {
            detail: { host: "games.roblox.com", retryInMs },
          }),
        ),
    );
  announce(300);
  announce(30); // a shorter wait must not cut the 300 ms one short
  expect(result.current).toBe(true);
  act(() => void vi.advanceTimersByTime(100));
  expect(result.current).toBe(true);
  act(() => void vi.advanceTimersByTime(250));
  expect(result.current).toBe(false);
}

function Strip() {
  const rove = useRoving(3);
  return (
    <div onKeyDown={rove.onKeyDown}>
      {["a", "b", "c"].map((n, i) => (
        <button key={n} type="button" {...rove.item(i)}>
          {n}
        </button>
      ))}
    </div>
  );
}

test("strip: one Tab stop, arrows and Home/End move focus, ends clamp", () => {
  render(<Strip />);
  const [a, b, c] = screen.getAllByRole("button");
  expect(a).toHaveAttribute("tabindex", "0");
  expect(b).toHaveAttribute("tabindex", "-1");
  act(() => a.focus());
  fireEvent.keyDown(a, { key: "ArrowRight" });
  expect(b).toHaveFocus();
  expect(b).toHaveAttribute("tabindex", "0");
  fireEvent.keyDown(b, { key: "End" });
  expect(c).toHaveFocus();
  fireEvent.keyDown(c, { key: "ArrowRight" });
  expect(c).toHaveFocus();
  fireEvent.keyDown(c, { key: "Home" });
  expect(a).toHaveFocus();
  fireEvent.keyDown(a, { key: "x" });
  expect(a).toHaveFocus();
});

test("sendToBar sets the Play bar; copy toasts success and failure", async () => {
  usePlayBar.setState({ destination: "" });
  useToasts.setState({ items: [] });
  sendToBar("555", "Obby");
  expect(usePlayBar.getState().destination).toBe("555");
  const writeText = vi
    .fn()
    .mockResolvedValueOnce(undefined)
    .mockRejectedValueOnce(new Error("denied"));
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  await copyText("42", "Place id");
  await copyText("42", "Place id");
  expect(writeText).toHaveBeenNthCalledWith(1, "42");
  await waitFor(() =>
    expect(useToasts.getState().items.map((x) => x.message)).toEqual([
      t("dest.set", { name: "Obby" }),
      t("copy.done", { what: "Place id" }),
      t("copy.failed", { reason: "denied" }),
    ]),
  );
});

test("Thumb: a failed image is dropped", () => {
  const { container } = render(<Thumb src="https://img.test/a.png" />);
  expect(container.querySelector("img")).toHaveAttribute("src", "https://img.test/a.png");
  fireEvent.error(container.querySelector("img")!);
  expect(container.querySelector("img")).toBeNull();
});

test("Universe: a bad id complains; a place id lists its universe's places", async () => {
  resetDiscoverMock(false);
  usePlayBar.setState({ destination: "" });
  const user = userEvent.setup();
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tooltip.Provider>
        <Universe />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  const input = screen.getByLabelText(t("universe.place"));
  await user.type(input, "abc");
  await user.click(screen.getByRole("button", { name: t("universe.lookup") }));
  expect(screen.getByText(t("universe.bad"))).toBeInTheDocument();
  await user.clear(input);
  await user.type(input, "74260430392611");
  await user.click(screen.getByRole("button", { name: t("universe.lookup") }));
  const list = await screen.findByRole("list", { name: t("universe.places") });
  expect(within(list).getAllByRole("button")).toHaveLength(5);
  expect(screen.queryByText(t("universe.bad"))).not.toBeInTheDocument();
});

test("Universe: takes the Play bar place when its tab opens with an empty input", () => {
  usePlayBar.setState({ destination: "" });
  useDiscover.setState({ tab: "games" });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tooltip.Provider>
        <Universe />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  const input = screen.getByLabelText(t("universe.place"));
  expect(input).toHaveValue("");
  act(() => usePlayBar.setState({ destination: "606849621" }));
  act(() => useDiscover.setState({ tab: "universe" }));
  expect(input).toHaveValue("606849621");
});

test("Outfits: unknown username shows the error; a valid one lists outfits", async () => {
  const user = userEvent.setup();
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <Tooltip.Provider>
        <Outfits />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  const input = screen.getByLabelText(t("outfits.username"));
  await user.type(input, "x");
  await user.click(screen.getByRole("button", { name: t("outfits.show") }));
  expect(await screen.findByText("No Roblox user has that name.")).toBeInTheDocument();
  await user.clear(input);
  await user.type(input, "Builder_1");
  await user.click(screen.getByRole("button", { name: t("outfits.show") }));
  expect(
    await screen.findByRole("list", { name: t("outfits.title", { name: "Builder_1" }) }),
  ).toBeInTheDocument();
  // the side panel's "Outfits of {name}" blurs in privacy mode
  expect(
    screen.getByText(t("outfits.title", { name: "Builder_1" }), { selector: "span" }),
  ).toHaveAttribute("data-pii");
});

test("Games search: no-match line, then Escape clears the box", async () => {
  resetDiscoverMock(false);
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tooltip.Provider>
        <Games />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  const box = screen.getByRole("searchbox", { name: t("games.search") });
  await userEvent.type(box, "zzqx");
  expect(
    await screen.findByText(t("games.noMatch", { q: "zzqx" }), undefined, { timeout: 2000 }),
  ).toBeInTheDocument();
  await userEvent.keyboard("{Escape}");
  expect(box).toHaveValue("");
});

test("Recents: Clear empties the list and says so", async () => {
  resetDiscoverMock(true);
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tooltip.Provider>
        <Favorites />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  const clear = await screen.findByRole("button", { name: t("recents.clear") });
  await waitFor(() => expect(clear).toBeEnabled());
  // the whole history: asks first
  await userEvent.click(clear);
  expect(screen.getByText(t("recents.clearAsk"))).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: t("recents.clear") }));
  expect(await screen.findByText(t("recents.empty"))).toBeInTheDocument();
});

test("Thumb: a new src after a failed one is tried again", () => {
  const { container, rerender } = render(<Thumb src="https://img.test/a.png" />);
  fireEvent.error(container.querySelector("img")!);
  rerender(<Thumb src="https://img.test/b.png" />);
  expect(container.querySelector("img")).toHaveAttribute("src", "https://img.test/b.png");
});
