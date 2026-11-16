import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { call, type GridLayout } from "@/lib/ipc";
import { mockVault, resetAccountsMock } from "@/lib/mock";
import { GridPopover } from "./GridPopover";
import { sameLayout, tiles, togglePick } from "./model";

const patch = (order: string[]) =>
  call("settings_set", {
    patch: { gridArrange: { monitor: null, order, layout: { type: "columns", count: 3 } } },
  });

beforeEach(async () => {
  resetAccountsMock(4);
  mockVault.lastArgs = {};
  await patch([]);
});

function setup(onOpenChange = vi.fn()) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <Tooltip.Provider>
        <GridPopover open onOpenChange={onOpenChange} />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
}

const row = (name: RegExp) => screen.findByRole("button", { name });

test("togglePick appends, unpick shifts later picks down", () => {
  let o = togglePick(togglePick(togglePick([], "a"), "b"), "c");
  expect(o).toEqual(["a", "b", "c"]);
  o = togglePick(o, "b");
  expect(o).toEqual(["a", "c"]);
});

test("tiles: one tile per window, main first", () => {
  for (const layout of [
    { type: "auto" },
    { type: "columns", count: 3 },
    { type: "cascade" },
    { type: "focus", main: "", others: "strip" },
    { type: "focus", main: "", others: "side" },
  ] as const)
    expect(tiles(layout, 7).map((t) => t.i)).toEqual([0, 1, 2, 3, 4, 5, 6]);
});

test("step 1: picking numbers accounts, unpicking renumbers", async () => {
  setup();
  expect(await screen.findByText("Pick your order")).toBeInTheDocument();
  const user = userEvent.setup();
  const pickers = await screen.findAllByRole("button", { pressed: false });
  const names = pickers.slice(0, 3);
  for (const b of names) await user.click(b);
  expect(screen.getAllByRole("button", { pressed: true })).toHaveLength(3);
  expect(screen.getByText("Main")).toBeInTheDocument();
  expect(screen.getByText("Alt 2")).toBeInTheDocument();
  await user.click(screen.getAllByRole("button", { pressed: true })[0]); // unpick the main
  expect(screen.getAllByRole("button", { pressed: true })).toHaveLength(2);
  expect(screen.queryByText("Alt 2")).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Clear" }));
  expect(screen.queryAllByRole("button", { pressed: true })).toHaveLength(0);
  expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
});

test("a saved order skips to step 2; Edit order goes back", async () => {
  await patch(["acc-2", "acc-1"]);
  setup();
  expect(await screen.findByText("Pick a layout")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Edit order" }));
  expect(await screen.findByText("Pick your order")).toBeInTheDocument();
  expect(screen.getAllByRole("button", { pressed: true })).toHaveLength(2);
});

test("Arrange sends order + layout and saves them as the default", async () => {
  const onOpenChange = vi.fn();
  await patch(["acc-2", "acc-1"]);
  setup(onOpenChange);
  const arrange = await row(/^Arrange$/);
  await waitFor(() => expect(screen.getAllByRole("radio").length).toBeGreaterThan(0));
  const cards = screen.getAllByRole("radio");
  expect(cards[0]).toHaveAttribute("aria-checked", "true");
  expect(screen.getByText("Best")).toBeInTheDocument();
  await userEvent.click(cards[cards.length - 1]);
  await userEvent.click(arrange);
  await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  const sent = mockVault.lastArgs.grid_arrange as { order: string[]; layout: { type: string } };
  expect(sent.order).toEqual(["acc-2", "acc-1"]);
  expect(sent.layout.type).toBeTruthy();
  await waitFor(() =>
    expect(mockVault.lastArgs.settings_set).toMatchObject({
      patch: { gridArrange: { order: ["acc-2", "acc-1"], layout: sent.layout } },
    }),
  );
});

test("Running marks come from open windows, not presence", async () => {
  setup();
  await screen.findByText("Pick your order");
  await waitFor(() => expect(screen.getAllByText("Running")).toHaveLength(2)); // acc-1, acc-2
});

test("layout count = picked accounts with a window + unknown windows (what grid_arrange tiles)", async () => {
  await patch(["acc-1"]); // mock windows: acc-1, acc-2 + 1 unknown; unpicked acc-2 stays put
  setup();
  await screen.findByText("Pick a layout");
  await waitFor(() => expect(mockVault.lastArgs.grid_suggest).toEqual({ count: 2 }));
});

test("sameLayout: type + own fields, ignores key order and stale merged keys", () => {
  expect(sameLayout({ type: "columns", count: 3 }, { count: 3, type: "columns" })).toBe(true);
  const stale = { type: "cascade", count: 3 } as unknown as GridLayout;
  expect(sameLayout({ type: "cascade" }, stale)).toBe(true);
  expect(sameLayout({ type: "columns", count: 3 }, { type: "columns", count: 2 })).toBe(false);
  expect(
    sameLayout(
      { type: "rows", counts: [2, 1], align: "center" },
      {
        align: "center",
        counts: [2, 1],
        type: "rows",
      },
    ),
  ).toBe(true);
  expect(
    sameLayout(
      { type: "focus", main: "", others: "side" },
      { type: "focus", main: "", others: "strip" },
    ),
  ).toBe(false);
  expect(sameLayout({ type: "auto" }, undefined)).toBe(false);
  expect(sameLayout({ type: "auto" }, { type: "auto" })).toBe(true);
  expect(sameLayout({ type: "rows", counts: [2, 1] }, { type: "rows", counts: [1, 2] })).toBe(
    false,
  );
  expect(
    sameLayout({ type: "rows", counts: [2] }, { type: "rows", counts: [2], align: "left" }),
  ).toBe(true);
  expect(
    sameLayout(
      { type: "focus", main: "acc-1", others: "side" },
      { type: "focus", main: "acc-2", others: "side" },
    ),
  ).toBe(false);
});

test("tiles: zero windows is empty; empty rows fall back to one row", () => {
  expect(tiles({ type: "single" }, 0)).toEqual([]);
  expect(tiles({ type: "rows", counts: [] }, 3).map((t) => t.y)).toEqual([0, 0, 0]);
  // columns 0 clamps to one column
  expect(tiles({ type: "columns", count: 0 }, 2).map((t) => t.y)).toEqual([0, 0.5]);
});

test("tiles: pyramid 1-2-1 for four, auto puts four in a 2x2", () => {
  expect(tiles({ type: "pyramid" }, 4).map((t) => [t.x, t.y])).toEqual([
    [0.25, 0],
    [0, 1 / 3],
    [0.5, 1 / 3],
    [0.25, 2 / 3],
  ]);
  expect(tiles({ type: "auto" }, 4).map((t) => [t.x, t.y, t.w, t.h])).toEqual([
    [0, 0, 0.5, 0.5],
    [0.5, 0, 0.5, 0.5],
    [0, 0.5, 0.5, 0.5],
    [0.5, 0.5, 0.5, 0.5],
  ]);
});

test("tiles: wide puts main 60% left, the rest stacked right", () => {
  expect(tiles({ type: "wide" }, 3)).toEqual([
    { i: 0, x: 0, y: 0, w: 0.6, h: 1 },
    { i: 1, x: 0.6, y: 0, w: 0.4, h: 0.5 },
    { i: 2, x: 0.6, y: 0.5, w: 0.4, h: 0.5 },
  ]);
});

test("tiles: focus minimize/behind/one window show only the main", () => {
  expect(tiles({ type: "focus", main: "", others: "minimize" }, 3)).toHaveLength(1);
  expect(tiles({ type: "focus", main: "", others: "behind" }, 3)).toHaveLength(1);
  expect(tiles({ type: "focus", main: "", others: "side" }, 1)).toHaveLength(1);
});

test("tiles: focus side and strip geometry", () => {
  expect(tiles({ type: "focus", main: "", others: "side" }, 3)).toEqual([
    { i: 0, x: 0, y: 0, w: 0.55, h: 1 },
    { i: 1, x: 0.55, y: 0, w: 0.45, h: 0.5 },
    { i: 2, x: 0.55, y: 0.5, w: 0.45, h: 0.5 },
  ]);
  const strip = tiles({ type: "focus", main: "", others: "strip" }, 4);
  expect(strip[0]).toMatchObject({ x: 0, y: 0, w: 1, h: 0.68 });
  expect(strip.slice(1).map((t) => [t.y, t.h])).toEqual([
    [0.68, 0.32],
    [0.68, 0.32],
    [0.68, 0.32],
  ]);
});

test("tiles: focus corner stacks small ones bottom-right, capped height", () => {
  const corner = tiles({ type: "focus", main: "", others: "corner" }, 4);
  expect(corner[1].x).toBeCloseTo(0.8);
  expect(corner[1].y + corner[1].h).toBeCloseTo(1); // bottom one sits on the bottom edge
  // five small ones: 0.9 / 5 is under the 0.22 cap
  const many = tiles({ type: "focus", main: "", others: "corner" }, 6);
  expect(many[1].h).toBeCloseTo(0.18);
});

test("tiles: cascade ends at the bottom-right corner", () => {
  expect(tiles({ type: "cascade" }, 1)).toEqual([{ i: 0, x: 0, y: 0, w: 1, h: 1 }]);
  const last = tiles({ type: "cascade" }, 3).at(-1)!;
  expect(last.x + last.w).toBeCloseTo(1);
  expect(last.y + last.h).toBeCloseTo(1);
});

test("the remembered layout is preselected even after a deep merge left stale keys", async () => {
  // columns 3 default merged with a cascade save: Rust/JS deep merge keeps count
  await call("settings_set", {
    patch: { gridArrange: { monitor: null, order: ["acc-1"], layout: { type: "cascade" } } },
  });
  setup();
  await screen.findByText("Pick a layout");
  // count settles at 3 once grid_windows resolves; Cascade is on offer then
  await waitFor(() =>
    expect(screen.getByRole("radio", { name: /cascade/i })).toHaveAttribute("aria-checked", "true"),
  );
});

test("tiles: focus side with 4+ others stacks them two wide", () => {
  const tile = tiles({ type: "focus", main: "", others: "side" }, 6);
  expect(tile.map((k) => k.i)).toEqual([0, 1, 2, 3, 4, 5]);
  const xs = [0.55, 0.775, 0.55, 0.775, 0.6625];
  tile.slice(1).forEach((k, j) => expect(k.x).toBeCloseTo(xs[j]));
});

test("sameLayout: rows align must match (missing means left); pyramid by type", () => {
  expect(
    sameLayout({ type: "rows", counts: [2], align: "center" }, { type: "rows", counts: [2] }),
  ).toBe(false);
  expect(sameLayout({ type: "pyramid" }, { type: "pyramid" })).toBe(true);
});
