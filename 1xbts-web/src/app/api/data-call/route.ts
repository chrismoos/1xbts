import { selectorFromUrl } from "@/lib/cell";
import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

export async function POST(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const body = await request.json();
    await waitForManagementReady();
    const client = getNetworkManagementClient();
    const result = await client.initiateDataCall(
      {
        selector: selectorFromUrl(request.url),
        request: {
          subscriberId: body.subscriberId || "",
          serviceOption: body.serviceOption ?? 33,
        },
      },
      { signal: abort.signal }
    );
    return Response.json(result);
  } catch (err) {
    return apiError(err, "start the data call", { accepted: false });
  } finally {
    clearTimeout(timeout);
  }
}
