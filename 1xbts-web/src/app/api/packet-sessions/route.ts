import { getPcfManagementClient, waitForBscReady } from "@/lib/grpc/client";
import { packetSessionToJson } from "@/lib/grpc/packet-session-json";

export const dynamic = "force-dynamic";

export async function GET() {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    await waitForBscReady();
    const client = getPcfManagementClient();
    const result = await client.listPcfSessions({}, { signal: abort.signal });
    return Response.json({
      ...result,
      sessions: result.sessions.map(packetSessionToJson),
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
