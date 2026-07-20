import { getAnClient } from "@/lib/grpc/an-client";

export const dynamic = "force-dynamic";

export async function GET() {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    const client = getAnClient();
    // Fetch UATI pool capacity/in-use/free stats. colorCode 0 selects the AN's
    // configured allocator; the response includes its actual sector color code.
    const result = await client.getUatiAllocation(
      { colorCode: 0 },
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
