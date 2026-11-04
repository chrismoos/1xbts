import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { packetSessionToJson } from "@/lib/grpc/packet-session-json";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ id: string }> }
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { id } = await params;
    await waitForManagementReady();
    const client = getNetworkManagementClient();
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
    return apiError(err, "load the packet session");
  } finally {
    clearTimeout(timeout);
  }
}
