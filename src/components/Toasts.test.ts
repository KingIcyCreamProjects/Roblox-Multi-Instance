import { act, fireEvent, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { heldToast, toast, Toaster, useToasts } from "./Toasts";

test("plain and action toasts each cap at 3; plain ones never push out an Undo", () => {
  useToasts.setState({ items: [] });
  toast("undo me", { label: "Undo", run: () => {} });
  for (const m of ["a", "b", "c", "d"]) toast(m);
  expect(useToasts.getState().items.map((x) => x.message)).toEqual(["undo me", "b", "c", "d"]);
  for (const m of ["u2", "u3", "u4"]) toast(m, { label: "Undo", run: () => {} });
  expect(useToasts.getState().items.map((x) => x.message)).toEqual([
    "b",
    "c",
    "d",
    "u2",
    "u3",
    "u4",
  ]);
});

test("toast() drops while locked; heldToast still queues", () => {
  useToasts.setState({ items: [], locked: true });
  toast("gone");
  expect(useToasts.getState().items).toEqual([]);
  heldToast("kept");
  expect(useToasts.getState().items.map((x) => x.message)).toEqual(["kept"]);
  useToasts.setState({ locked: false });
});

test("default timers: 4 s plain, 8 s with action; a returned dismiss removes only its toast", () => {
  useToasts.setState({ items: [], locked: false });
  const dismiss = toast("plain");
  toast("act", { label: "Undo", run: () => {} });
  const [p, a] = useToasts.getState().items;
  expect([p.ms, a.ms]).toEqual([4000, 8000]);
  dismiss();
  expect(useToasts.getState().items.map((x) => x.message)).toEqual(["act"]);
});

test("a toast dismisses after its ms; hover pauses the countdown, leave resumes it", () => {
  vi.useFakeTimers();
  try {
    useToasts.setState({ items: [], locked: false });
    render(createElement(Toaster));
    act(() => void toast("bye", undefined, 1000));
    act(() => vi.advanceTimersByTime(500));
    fireEvent.mouseEnter(screen.getByText("bye"));
    act(() => vi.advanceTimersByTime(5000));
    expect(useToasts.getState().items).toHaveLength(1);
    fireEvent.mouseLeave(screen.getByText("bye"));
    act(() => vi.advanceTimersByTime(499));
    expect(useToasts.getState().items).toHaveLength(1);
    act(() => vi.advanceTimersByTime(1));
    expect(useToasts.getState().items).toHaveLength(0);
  } finally {
    vi.useRealTimers();
  }
});
