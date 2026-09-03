import { peerRequestFromUrl, selectorFromUrl } from "@/lib/cell";
import { getNetworkManagementClient } from "@/lib/grpc/client";
import { GetSessionResponse } from "@/lib/proto/an/v1/service";

export const dynamic = "force-dynamic";

export async function GET(
  req: Request,
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
    const client = getNetworkManagementClient();
    const result = await client.getAnSession(
      {
        selector: selectorFromUrl(req.url),
        cell: peerRequestFromUrl(req.url),
        request: { uati: uatiNum },
      },
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
