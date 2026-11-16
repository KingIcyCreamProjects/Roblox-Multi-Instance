import { useQuery } from "@tanstack/react-query";
import { call, type LaunchTarget } from "@/lib/ipc";
import { destText } from "./model";

/** Game name of a saved target (same cached resolve as the DestChip); undefined until known. */
export function useDestName(d: LaunchTarget | null | undefined) {
  const input = d?.privateLink ?? (d?.placeId ? String(d.placeId) : "");
  const { data } = useQuery({
    queryKey: ["dest", input],
    queryFn: () => call("destination_resolve", { input }),
    enabled: !!input,
    staleTime: 60_000,
    meta: { silent: true },
  });
  return data?.name ?? undefined;
}

/** A saved destination as text: game name, plus private / server / mode. */
export function DestLabel({ d }: { d: LaunchTarget | null | undefined }) {
  return <>{destText(d, useDestName(d))}</>;
}
