import { getAnClient } from "@/lib/grpc/an-client";
import { GetSessionsResponse, SessionState } from "@/lib/proto/an/v1/service";

export const dynamic = "force-dynamic";

export async function GET() {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    const client = getAnClient();
    const result = await client.getSessions(
      { stateFilter: SessionState.SESSION_STATE_UNSPECIFIED },
      { signal: abort.signal },
    );
    return Response.json(GetSessionsResponse.toJSON(result));
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
