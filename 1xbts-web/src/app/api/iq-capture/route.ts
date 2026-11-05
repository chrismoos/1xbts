import { cellScopedRequestFromUrl } from "@/lib/cell";
import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

async function withClient<T>(
  action: string,
  call: (client: ReturnType<typeof getNetworkManagementClient>, signal: AbortSignal) => Promise<T>
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    await waitForManagementReady();
    const client = getNetworkManagementClient();
    return Response.json(await call(client, abort.signal));
  } catch (err) {
    return apiError(err, action);
  } finally {
    clearTimeout(timeout);
  }
}

export async function GET(request: Request) {
  const req = cellScopedRequestFromUrl(request.url);
  return withClient("load the IQ capture status", (client, signal) => client.getIqCaptureStatus(req, { signal }));
}

export async function POST(request: Request) {
  const req = cellScopedRequestFromUrl(request.url);
  return withClient("start the IQ capture", (client, signal) => client.startIqCapture(req, { signal }));
}

export async function DELETE(request: Request) {
  const req = cellScopedRequestFromUrl(request.url);
  return withClient("stop the IQ capture", (client, signal) => client.stopIqCapture(req, { signal }));
}
