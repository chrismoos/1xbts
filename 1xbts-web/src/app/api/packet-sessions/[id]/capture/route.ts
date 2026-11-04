import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

async function setCapture(
  id: string,
  enabled: boolean,
  signal: AbortSignal
) {
  await waitForManagementReady();
  const client = getNetworkManagementClient();
  return client.setPacketTraceCapture({ sessionId: id, enabled }, { signal });
}

export async function POST(
  _request: Request,
  { params }: { params: Promise<{ id: string }> }
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { id } = await params;
    const result = await setCapture(id, true, abort.signal);
    return Response.json(result);
  } catch (err) {
    return apiError(err, "start the packet capture");
  } finally {
    clearTimeout(timeout);
  }
}

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ id: string }> }
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { id } = await params;
    const result = await setCapture(id, false, abort.signal);
    return Response.json(result);
  } catch (err) {
    return apiError(err, "stop the packet capture");
  } finally {
    clearTimeout(timeout);
  }
}
