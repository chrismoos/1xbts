import { cellScopedRequestFromUrl } from "@/lib/cell";
import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

async function withClient<T>(
  action: (client: ReturnType<typeof getNetworkManagementClient>, signal: AbortSignal) => Promise<T>
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    await waitForManagementReady();
    const client = getNetworkManagementClient();
    return Response.json(await action(client, abort.signal));
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}

export async function GET(request: Request) {
  const req = cellScopedRequestFromUrl(request.url);
  return withClient((client, signal) => client.getIqCaptureStatus(req, { signal }));
}

export async function POST(request: Request) {
  const req = cellScopedRequestFromUrl(request.url);
  return withClient((client, signal) => client.startIqCapture(req, { signal }));
}

export async function DELETE(request: Request) {
  const req = cellScopedRequestFromUrl(request.url);
  return withClient((client, signal) => client.stopIqCapture(req, { signal }));
}
