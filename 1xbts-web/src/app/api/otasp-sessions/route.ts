import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";
import { ListOtaspSessionsResponse } from "@/lib/proto/hlr/v1/service";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    const { searchParams } = new URL(request.url);
    const limit = Number(searchParams.get("limit") ?? "10");
    const offset = Number(searchParams.get("offset") ?? "0");
    const subscriberIdRaw = searchParams.get("subscriberId");
    const esnRaw = searchParams.get("esn");
    const meidRaw = searchParams.get("meid");
    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.listOtaspSessions(
      {
        limit,
        offset,
        subscriberId: subscriberIdRaw ?? undefined,
        esn: esnRaw != null ? Number(esnRaw) : undefined,
        meid: meidRaw ?? undefined,
      },
      { signal: abort.signal }
    );
    // Run proto toJSON so timestamps render as ISO strings and bytes
    // fields encode as base64 — matches the convention the rest of the
    // app uses.
    const json = ListOtaspSessionsResponse.toJSON(result) as Record<
      string,
      unknown
    >;
    return Response.json({
      sessions: json.sessions ?? [],
      total: json.total ?? 0,
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
