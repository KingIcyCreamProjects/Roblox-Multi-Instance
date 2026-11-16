import { emptySel } from "@/features/accounts/model";
import { useUi } from "./store";

beforeEach(() => {
  useUi.setState({
    route: "accounts",
    selGroup: null,
    sel: emptySel,
    add: { ...useUi.getState().add, open: false },
  });
});

test("setSel clears the group header selection", () => {
  useUi.setState({ selGroup: "Alts" });
  useUi.getState().setSel({ ids: ["a"], anchor: "a", cursor: "a" });
  expect(useUi.getState()).toMatchObject({ selGroup: null, sel: { ids: ["a"] } });
});

test("selectGroupOf: cursor on first row unless collapsed; empty group has no anchor", () => {
  useUi.getState().selectGroupOf("Main", ["x", "y"]);
  expect(useUi.getState()).toMatchObject({
    selGroup: "Main",
    sel: { ids: ["x", "y"], anchor: "x", cursor: "x" },
  });
  useUi.getState().selectGroupOf("Hidden", ["z"], true);
  expect(useUi.getState().sel).toMatchObject({ anchor: "z", cursor: null });
  useUi.getState().selectGroupOf("Empty", []);
  expect(useUi.getState().sel).toMatchObject({ ids: [], anchor: null, cursor: null });
});

test("openAdd: prefillFor only with a prefill; nonce bumps on every open", () => {
  useUi.getState().openAdd();
  const first = useUi.getState().add;
  expect(first).toMatchObject({ open: true, tab: "signin", prefill: "", prefillFor: null });
  useUi.getState().openAdd("cookies", "abc");
  expect(useUi.getState().add).toMatchObject({
    tab: "cookies",
    prefill: "abc",
    prefillFor: "cookies",
    nonce: first.nonce + 1,
  });
});

test("setError: waiting queue keeps the newest 5; dismiss shows the next", () => {
  useUi.setState({ lastError: null, moreErrors: [] });
  const { setError } = useUi.getState();
  for (const e of ["e0", "e1", "e2", "e3", "e4", "e5", "e6"]) setError(e);
  expect(useUi.getState()).toMatchObject({
    lastError: "e0",
    moreErrors: ["e2", "e3", "e4", "e5", "e6"],
  });
  setError(null);
  expect(useUi.getState()).toMatchObject({ lastError: "e2", moreErrors: ["e3", "e4", "e5", "e6"] });
});
