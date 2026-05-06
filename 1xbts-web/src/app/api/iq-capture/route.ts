import { getBtsManagementClient, waitForBscReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

async function withClient<T>(
  action: (client: ReturnType<typeof getBtsManagementClient>, signal: AbortSignal) => Promise<T>
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    await waitForBscReady();
    const client = getBtsManagementClient();
    return Response.json(await action(client, abort.signal));
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}

export async function GET() {
  return withClient((client, signal) => client.getIqCaptureStatus({}, { signal }));
}

export async function POST() {
  return withClient((client, signal) => client.startIqCapture({}, { signal }));
}

export async function DELETE() {
  return withClient((client, signal) => client.stopIqCapture({}, { signal }));
}
