import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { merge, useSetSettings } from "./useSettings";

// The save never answers, so the cache can only hold the optimistic value.
vi.mock("@/lib/ipc", async () => ({
  ...(await vi.importActual<typeof import("@/lib/ipc")>("@/lib/ipc")),
  call: () => new Promise(() => {}),
}));

test("settings patch merges nested objects, replaces leaves and arrays", () => {
  const s = { ui: { showUsernames: true, scale: 100 }, list: [1, 2], n: 1 };
  expect(merge(s, { ui: { showUsernames: false }, list: [3] })).toEqual({
    ui: { showUsernames: false, scale: 100 },
    list: [3],
    n: 1,
  });
});

test("a save shows its patch in the cache before the server answers", async () => {
  const qc = new QueryClient();
  qc.setQueryData(["settings"], { ui: { showUsernames: true, scale: 100 } });
  const wrapper = ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client: qc }, children);
  const { result } = renderHook(() => useSetSettings(), { wrapper });
  result.current.mutate({ ui: { scale: 110 } });
  await waitFor(() =>
    expect(qc.getQueryData(["settings"])).toEqual({ ui: { showUsernames: true, scale: 110 } }),
  );
});
