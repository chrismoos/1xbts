import { getMscManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { apiError, badRequest } from "@/lib/api-error";

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
      return badRequest("The call ID is not valid", "callId");
    }
    await waitForManagementReady();
    await getMscManagementClient().releaseCall({ callId: id }, { signal: abort.signal });
    return Response.json({ released: true });
  } catch (err) {
    return apiError(err, "end the call");
  } finally {
    clearTimeout(timeout);
  }
}
