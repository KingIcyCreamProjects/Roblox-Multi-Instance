import { onlineManager } from "@tanstack/react-query";
import { queryClient } from "./queryClient";

// Every query is local Tauri IPC: a browser "offline" event must not pause it.
test("queries and mutations run while the browser says offline", async () => {
  onlineManager.setOnline(false);
  try {
    await expect(queryClient.fetchQuery({ queryKey: ["t"], queryFn: () => 1 })).resolves.toBe(1);
    const m = queryClient.getMutationCache().build(queryClient, { mutationFn: async () => 2 });
    await expect(m.execute(undefined)).resolves.toBe(2);
  } finally {
    onlineManager.setOnline(true);
    queryClient.clear();
  }
});
