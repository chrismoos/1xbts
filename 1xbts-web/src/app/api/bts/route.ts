import { selectorFromUrl } from "@/lib/cell";
import {
  getMscManagementClient,
  getNetworkManagementClient,
  waitForManagementReady,
} from "@/lib/grpc/client";
import type { BtsSummary } from "@/lib/proto/bts_management/v1/service";

export const dynamic = "force-dynamic";

// Each returned cell carries the base station it belongs to, so the UI can
// address it unambiguously (a cell/sector token is only unique within one base
// station).
type TaggedCell = BtsSummary & { baseStation: string };

export async function GET(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    await waitForManagementReady();
    const net = getNetworkManagementClient();
    const baseStation = selectorFromUrl(request.url).baseStation;

    // A request naming a base station lists just its cells. Otherwise list
    // every base station's cells so the whole network's cells are addressable.
    const targets = baseStation
      ? [baseStation]
      : (await getMscManagementClient().listBaseStations({}, { signal: abort.signal })).nodes.map(
          (node) => node.managementEndpoint,
        );

    const bts: TaggedCell[] = [];
    for (const target of targets) {
      try {
        const result = await net.listBts(
          { selector: { baseStation: target } },
          { signal: abort.signal },
        );
        for (const cell of result.bts) {
          bts.push({ ...cell, baseStation: target });
        }
      } catch {
        // Skip a base station that is offline.
      }
    }
    return Response.json({ bts });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
