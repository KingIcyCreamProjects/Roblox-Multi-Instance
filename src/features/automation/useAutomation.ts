import { useQuery } from "@tanstack/react-query";
import { call, inTauri } from "@/lib/ipc";

/** Snapshot of all controlled accounts. Rust sends state://changed ["snap"] on every hub change,
 * engine reset and lock (useStateSync, app-wide); polled 1 s in the dev mock. */
// ponytail: refetches every account's console tail per change; switch to a per-account
// snapshot command if many chatty scripts make this heavy.
export function useSnapshot() {
  return useQuery({
    queryKey: ["snap"],
    queryFn: () => call("automation_snapshot"),
    refetchInterval: inTauri ? false : 1000,
  });
}

/** Server status. Rust sends state://changed ["autoStatus"] on bind, stop and a server task ending. */
export function useAutomationStatus() {
  return useQuery({
    queryKey: ["autoStatus"],
    queryFn: () => call("automation_status"),
    refetchInterval: inTauri ? false : 1000,
  });
}

export const useToken = () =>
  useQuery({ queryKey: ["autoToken"], queryFn: () => call("automation_token") });
