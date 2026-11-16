import { barOf, clearPlaceholder, launchBusy, useLaunch } from "./usePlay";

test("a settled (done) batch does not block the next play", () => {
  const b = { id: "b1", ids: ["a"], done: null };
  useLaunch.setState({ batch: b });
  expect(launchBusy()).toBe(true);
  useLaunch.setState({ batch: { ...b, done: { batchId: "b1" } as never } });
  expect(launchBusy()).toBe(false);
  useLaunch.setState({ batch: null });
});

test("barOf drops a leftover server when the destination is empty", () => {
  expect(barOf("", " job ", "specific")).toEqual({
    destination: "",
    server: "",
    mode: "specific",
    home: true,
  });
  expect(barOf(" 1818 ", " job ", "specific")).toEqual({
    destination: "1818",
    server: "job",
    mode: "specific",
    home: true,
  });
});

test("a placeholder replaced meanwhile (cancel) is still cleared, and a real batch is adopted", () => {
  const ids = ["a"];
  useLaunch.setState({
    batch: { id: null, ids, done: null, cancel: true },
    states: { b9: { a: "launching" } },
  });
  expect(launchBusy()).toBe(true);
  clearPlaceholder(ids);
  expect(useLaunch.getState().batch?.id).toBe("b9"); // adopted like settle does
  useLaunch.setState({ batch: null, states: {} });
});
