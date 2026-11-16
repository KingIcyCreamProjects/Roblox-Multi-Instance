import { MutationCache, QueryCache, QueryClient } from "@tanstack/react-query";
import { reportError } from "./report";

export const queryClient = new QueryClient({
  // networkMode "always": every call is local IPC, so a browser "offline" event must not pause it
  defaultOptions: {
    queries: { staleTime: Infinity, retry: false, networkMode: "always" },
    mutations: { networkMode: "always" },
  },
  queryCache: new QueryCache({ onError: (e, q) => q.meta?.silent || reportError(e) }),
  mutationCache: new MutationCache({
    // a mutation with its own onError reports for itself (no double toast + banner)
    onError: (e, _v, _c, m) => {
      if (!m.options.onError) reportError(e);
    },
  }),
});
