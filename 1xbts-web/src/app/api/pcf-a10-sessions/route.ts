import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { ListA10SessionsResponse } from "@/lib/proto/pcf/v1/service";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

export async function GET() {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    await waitForManagementReady();
    const result = await getNetworkManagementClient().listPcfA10Sessions({}, { signal: abort.signal });
    return Response.json(ListA10SessionsResponse.toJSON(result));
  } catch (err) {
    return apiError(err, "load PCF sessions");
  } finally {
    clearTimeout(timeout);
  }
}
