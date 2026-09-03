import { selectorFromUrl } from "@/lib/cell";
import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    await waitForManagementReady();
    const client = getNetworkManagementClient();
    // A named base station lists its own channels. Without one, the network-
    // wide dashboard aggregates every base station.
    const selector = selectorFromUrl(request.url);
    const result = selector.baseStation
      ? await client.listChannels({ selector }, { signal: abort.signal })
      : await client.listAllChannels({}, { signal: abort.signal });
    return Response.json(result);
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
