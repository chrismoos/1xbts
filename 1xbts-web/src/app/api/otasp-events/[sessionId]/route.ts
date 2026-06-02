import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";
import { GetOtaspSessionResponse } from "@/lib/proto/hlr/v1/service";

export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ sessionId: string }> },
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { sessionId } = await params;
    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.getOtaspSession(
      { sessionId },
      { signal: abort.signal },
    );
    if (!result.session) {
      return Response.json({ error: "session not found" }, { status: 404 });
    }
    // Use proto toJSON so Uint8Array bytes fields (e.g. PRL raw_bytes
    // embedded in the OtaspPrlReadback event) encode as base64 strings
    // rather than `{0: 78, 1: 79, ...}` objects from naive JSON.stringify.
    const json = GetOtaspSessionResponse.toJSON(result) as Record<
      string,
      unknown
    >;
    return Response.json({ session: json.session });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = msg.toLowerCase().includes("not found") ? 404 : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}
