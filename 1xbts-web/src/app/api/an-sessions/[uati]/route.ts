import { getAnClient } from "@/lib/grpc/an-client";
import { GetSessionResponse } from "@/lib/proto/an/v1/service";

export const dynamic = "force-dynamic";

export async function GET(
  _req: Request,
  { params }: { params: Promise<{ uati: string }> },
) {
  const { uati } = await params;
  const uatiNum = Number.parseInt(uati, 16);
  if (!Number.isFinite(uatiNum)) {
    return Response.json({ error: "invalid uati" }, { status: 400 });
  }
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    const client = getAnClient();
    const result = await client.getSession(
      { uati: uatiNum },
      { signal: abort.signal },
    );
    return Response.json(GetSessionResponse.toJSON(result));
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
