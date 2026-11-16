import * as Dialog from "@radix-ui/react-dialog";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { t } from "@/lib/i18n";
import { resetAccountsMock } from "@/lib/mock";
import { Palette } from "./Palette";
import { useUi } from "./store";

const wrap = (other: boolean) =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Palette />
      <Dialog.Root open={other}>
        <Dialog.Portal>
          <Dialog.Content aria-describedby={undefined}>
            <Dialog.Title>Other</Dialog.Title>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </QueryClientProvider>,
  );

beforeEach(() => useUi.setState({ paletteOpen: false }));

test("Ctrl+K opens the palette, but not over another dialog", async () => {
  const u = userEvent.setup();
  const { unmount } = wrap(true);
  await u.keyboard("{Control>}k{/Control}");
  expect(useUi.getState().paletteOpen).toBe(false);
  unmount();
  wrap(false);
  await u.keyboard("{Control>}k{/Control}");
  expect(useUi.getState().paletteOpen).toBe(true);
  expect(await screen.findByRole("listbox")).toBeInTheDocument();
});

test("unmounting (lock) closes the palette", () => {
  const { unmount } = wrap(false);
  useUi.setState({ paletteOpen: true });
  unmount();
  expect(useUi.getState().paletteOpen).toBe(false);
});

test("closing with Ctrl+K clears the query for the next open", async () => {
  const u = userEvent.setup();
  wrap(false);
  await u.keyboard("{Control>}k{/Control}");
  await u.type(await screen.findByRole("combobox"), "set");
  await u.keyboard("{Control>}k{/Control}");
  expect(useUi.getState().paletteOpen).toBe(false);
  await u.keyboard("{Control>}k{/Control}");
  expect(await screen.findByRole("combobox")).toHaveValue("");
});

test("an aliased account is found by its username", async () => {
  const u = userEvent.setup();
  resetAccountsMock(6);
  wrap(false);
  await u.keyboard("{Control>}k{/Control}");
  await u.type(await screen.findByRole("combobox"), "kestrel_rbx");
  expect(
    await screen.findByRole("option", {
      name: new RegExp(t("palette.play", { name: "Kestrel main" })),
    }),
  ).toBeInTheDocument();
});

test("arrow keys keep the highlighted command scrolled into view", async () => {
  const u = userEvent.setup();
  const spy = vi.fn();
  Element.prototype.scrollIntoView = spy;
  wrap(false);
  await u.keyboard("{Control>}k{/Control}");
  await screen.findByRole("combobox");
  spy.mockClear();
  await u.keyboard("{ArrowDown}");
  expect(spy).toHaveBeenCalledWith({ block: "nearest" });
});

test("opening one modal closes any other", () => {
  useUi.setState({ toolsOpen: true });
  useUi.setState({ paletteOpen: true });
  expect(useUi.getState()).toMatchObject({ toolsOpen: false, paletteOpen: true });
  useUi.getState().openAdd();
  expect(useUi.getState()).toMatchObject({ paletteOpen: false, add: { open: true } });
  useUi.setState((u) => ({ add: { ...u.add, open: false } }));
});

test("errors queue behind the banner; dismiss shows the next", () => {
  useUi.setState({ lastError: null, moreErrors: [] });
  const { setError } = useUi.getState();
  for (const e of ["a", "b", "a", "c"]) setError(e);
  expect(useUi.getState()).toMatchObject({ lastError: "a", moreErrors: ["b", "c"] });
  setError(null);
  expect(useUi.getState()).toMatchObject({ lastError: "b", moreErrors: ["c"] });
});
