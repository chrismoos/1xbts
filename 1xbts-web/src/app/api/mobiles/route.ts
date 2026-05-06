import { getBscManagementClient, waitForBscReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

export async function GET() {
  const abort = AbortController.prototype
    ? new AbortController()
    : { signal: undefined, abort() {} };
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    console.log("[mobiles] gRPC call");
    await waitForBscReady();
    const client = getBscManagementClient();
    const result = await client.listMobiles({}, { signal: abort.signal });
    console.log(`[mobiles] ok (${result.mobiles.length} mobiles)`);
    return Response.json(result.mobiles);
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    console.log(`[mobiles] gRPC error: ${msg}`);
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
