import { peerRequestFromUrl, selectorFromUrl } from "@/lib/cell";
import { getNetworkManagementClient } from "@/lib/grpc/client";
import { GetSessionResponse } from "@/lib/proto/an/v1/service";
import { apiError, badRequest } from "@/lib/api-error";

export const dynamic = "force-dynamic";

export async function GET(
  req: Request,
  { params }: { params: Promise<{ uati: string }> },
) {
  const { uati } = await params;
  const uatiNum = Number.parseInt(uati, 16);
  if (!Number.isFinite(uatiNum)) {
    return badRequest("The UATI is not valid", "uati");
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
    return apiError(err, "load the EV-DO session");
  } finally {
    clearTimeout(timeout);
  }
}
