import { peerRequestFromUrl, selectorFromUrl } from "@/lib/cell";
import { getNetworkManagementClient } from "@/lib/grpc/client";
import { evdoCells, isUnscoped } from "@/lib/grpc/an-aggregate";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    const client = getNetworkManagementClient();
    // colorCode 0 selects the AN's configured allocator. The response includes
    // its actual sector color code.
    // A request with no cell sums the UATI pool across every EV-DO cell.
    if (isUnscoped(request.url)) {
      let capacity = 0;
      let inUse = 0;
      let available = 0;
      for (const ref of await evdoCells(abort.signal)) {
        const r = await client.getAnUatiAllocation(
          { ...ref, request: { colorCode: 0 } },
          { signal: abort.signal },
        );
        capacity += r.capacity;
        inUse += r.inUse;
        available += r.available;
      }
      return Response.json({ colorCode: 0, capacity, inUse, available });
    }
    const result = await client.getAnUatiAllocation(
      {
        selector: selectorFromUrl(request.url),
        cell: peerRequestFromUrl(request.url),
        request: { colorCode: 0 },
      },
      { signal: abort.signal },
    );
    return Response.json(result);
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
