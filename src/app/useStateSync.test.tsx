import { QueryClient, QueryClientProvider, QueryObserver } from "@tanstack/react-query";
import { act, render, waitFor } from "@testing-library/react";
import { mockBus } from "@/lib/ipc";
import { KEY_SET } from "./useSettings";
import { STATE_CHANGED, useStateSync } from "./useStateSync";

function Sync() {
  useStateSync();
  return null;
}

test("state://changed invalidates every query under each named root, nothing else", () => {
  const qc = new QueryClient();
  for (const k of [["autoStatus"], ["pin", "a"], ["pin", "b"], ["settings"]]) qc.setQueryData(k, 1);
  render(
    <QueryClientProvider client={qc}>
      <Sync />
    </QueryClientProvider>,
  );
  act(
    () =>
      void mockBus.dispatchEvent(new CustomEvent(STATE_CHANGED, { detail: ["autoStatus", "pin"] })),
  );
  const stale = (k: unknown[]) => qc.getQueryState(k)?.isInvalidated;
  expect([["autoStatus"], ["pin", "a"], ["pin", "b"], ["settings"]].map(stale)).toEqual([
    true,
    true,
    true,
    false,
  ]);
});

test("a burst never cancels the fetch in flight; one trailing refetch picks up the last change", async () => {
  const qc = new QueryClient();
  const calls: { signal: AbortSignal; done: (v: number) => void }[] = [];
  const off = new QueryObserver(qc, {
    queryKey: ["snap"],
    staleTime: Infinity,
    initialData: 0,
    queryFn: ({ signal }) => new Promise<number>((done) => calls.push({ signal, done })),
  }).subscribe(() => {});
  render(
    <QueryClientProvider client={qc}>
      <Sync />
    </QueryClientProvider>,
  );
  const fire = () =>
    act(() => void mockBus.dispatchEvent(new CustomEvent(STATE_CHANGED, { detail: ["snap"] })));
  fire();
  fire();
  fire();
  expect(calls).toHaveLength(1);
  expect(calls[0]!.signal.aborted).toBe(false);
  await act(async () => calls[0]!.done(1));
  await waitFor(() => expect(calls).toHaveLength(2));
  await act(async () => calls[1]!.done(2));
  expect(calls).toHaveLength(2);
  expect(qc.getQueryData(["snap"])).toBe(2);
  off();
});

test("a settings save in flight keeps its optimistic value: state://changed skips settings", () => {
  const qc = new QueryClient();
  qc.setQueryData(["settings"], 1);
  render(
    <QueryClientProvider client={qc}>
      <Sync />
    </QueryClientProvider>,
  );
  const busy = vi.spyOn(qc, "isMutating").mockReturnValue(1);
  const fire = () =>
    act(() => void mockBus.dispatchEvent(new CustomEvent(STATE_CHANGED, { detail: ["settings"] })));
  fire();
  expect(busy).toHaveBeenCalledWith({ mutationKey: KEY_SET });
  expect(qc.getQueryState(["settings"])?.isInvalidated).toBe(false);
  busy.mockReturnValue(0);
  fire();
  expect(qc.getQueryState(["settings"])?.isInvalidated).toBe(true);
});
