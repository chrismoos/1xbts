import { getPdsnManagementClient, waitForBscReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

async function setCapture(
  id: string,
  enabled: boolean,
  signal: AbortSignal
) {
  await waitForBscReady();
  const client = getPdsnManagementClient();
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
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = msg.toLowerCase().includes("not found") ? 404 : 502;
    return Response.json({ error: msg }, { status });
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
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = msg.toLowerCase().includes("not found") ? 404 : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}
