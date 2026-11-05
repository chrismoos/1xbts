import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { packetSessionToJson } from "@/lib/grpc/packet-session-json";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

export async function GET() {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    await waitForManagementReady();
    const client = getNetworkManagementClient();
    const result = await client.listPcfSessions({}, { signal: abort.signal });
    return Response.json({
      ...result,
      sessions: result.sessions.map(packetSessionToJson),
    });
  } catch (err) {
    return apiError(err, "load packet sessions");
  } finally {
    clearTimeout(timeout);
  }
}
