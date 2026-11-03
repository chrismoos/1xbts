import { getMscManagementClient, waitForManagementReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ callId: string }> },
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { callId } = await params;
    const id = Number(callId);
    if (!Number.isSafeInteger(id) || id <= 0) {
      return Response.json({ error: "callId must be a positive integer" }, { status: 400 });
    }
    await waitForManagementReady();
    await getMscManagementClient().releaseCall({ callId: id }, { signal: abort.signal });
    return Response.json({ released: true });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = /NOT_FOUND/.test(msg) ? 404 : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}
