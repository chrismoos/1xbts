import { bsRequestFromUrl } from "@/lib/cell";
import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const abort = AbortController.prototype
    ? new AbortController()
    : { signal: undefined, abort() {} };
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    console.log("[system-status] gRPC call");
    await waitForManagementReady();
    const client = getNetworkManagementClient();
    const status = await client.getBscStatus(bsRequestFromUrl(request.url), {
      signal: abort.signal,
    });
    console.log("[system-status] ok");
    return Response.json(status);
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    console.log(`[system-status] gRPC error: ${msg}`);
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
