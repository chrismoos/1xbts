import { cellScopedRequestFromUrl } from "@/lib/cell";
import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    await waitForManagementReady();
    const client = getNetworkManagementClient();
    const result = await client.listReversePowerControls(
      cellScopedRequestFromUrl(request.url),
      { signal: abort.signal },
    );
    return Response.json(result);
  } catch (err) {
    return apiError(err, "load reverse power control");
  } finally {
    clearTimeout(timeout);
  }
}
