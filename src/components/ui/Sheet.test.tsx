import { render, renderHook, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { Sheet, useModalFocus } from "./Sheet";

function Harness() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        opener
      </button>
      <Sheet open={open} onOpenChange={setOpen} title="Sheet">
        body
      </Sheet>
    </>
  );
}

// opened from state (no Dialog.Trigger): Radix alone would drop focus on body
test("closing a sheet gives focus back to its opener", async () => {
  const u = userEvent.setup();
  render(<Harness />);
  const opener = screen.getByRole("button", { name: "opener" });
  await u.click(opener);
  await screen.findByRole("dialog");
  await u.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() => expect(opener).toHaveFocus());
});

test("pressing the title bar or a toast (data-modal-pass) does not count as outside", () => {
  const { result } = renderHook(() => useModalFocus());
  const press = (target: Element) => {
    const e = new Event("pointerdown", { cancelable: true });
    Object.defineProperty(e, "target", { value: target });
    result.current.onInteractOutside(e);
    return e.defaultPrevented;
  };
  const bar = document.createElement("header");
  bar.dataset.modalPass = "";
  bar.append(document.createElement("button"));
  expect(press(bar.firstElementChild!)).toBe(true);
  expect(press(document.createElement("main"))).toBe(false);
});
