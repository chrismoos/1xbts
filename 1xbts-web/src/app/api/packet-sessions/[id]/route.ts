import { getPcfManagementClient, waitForBscReady } from "@/lib/grpc/client";
import { packetSessionToJson } from "@/lib/grpc/packet-session-json";

export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ id: string }> }
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { id } = await params;
    await waitForBscReady();
    const client = getPcfManagementClient();
    const result = await client.getPcfSession(
      { sessionId: id },
      { signal: abort.signal }
    );
    const summary = result.session?.summary;
    return Response.json({
      ...result,
      session: result.session
        ? {
            ...result.session,
            summary: summary ? packetSessionToJson(summary) : summary,
          }
        : result.session,
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = msg.toLowerCase().includes("not found") ? 404 : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}
