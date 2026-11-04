import { cellScopedRequestFromUrl } from "@/lib/cell";
import { getNetworkManagementClient } from "@/lib/grpc/client";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    const client = getNetworkManagementClient();
    const result = await client.getBtsConfig(
      cellScopedRequestFromUrl(request.url),
      { signal: abort.signal },
    );
    return Response.json(result);
  } catch (err) {
    return apiError(err, "load the BTS config");
  } finally {
    clearTimeout(timeout);
  }
}
