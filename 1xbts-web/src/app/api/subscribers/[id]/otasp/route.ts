import { getMscManagementClient, waitForManagementReady, grpcErrorMessage, grpcErrorStatus } from "@/lib/grpc/client";
import { validateImsi } from "@/lib/validation";

const MANAGEMENT_TIMEOUT_MS = 5000;
const BAD_REQUEST_STATUS = 400;

export const dynamic = "force-dynamic";

export async function POST(request: Request, { params }: { params: Promise<{ id: string }> }) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), MANAGEMENT_TIMEOUT_MS);
  try {
    const { id } = await params;
    const body = await request.json().catch(() => null);
    if (!body || typeof body !== "object" || (body.imsi !== undefined && typeof body.imsi !== "string")) {
      return Response.json({ error: "A JSON object with an optional IMSI is required" }, { status: BAD_REQUEST_STATUS });
    }
    const imsi: string | undefined = body.imsi;
    if (imsi !== undefined) {
      const validation = validateImsi(imsi);
      if (!validation.ok) return Response.json({ error: validation.error }, { status: BAD_REQUEST_STATUS });
    }
    await waitForManagementReady();
    const result = await getMscManagementClient().initiateOtasp({ subscriberId: id, imsi }, { signal: abort.signal });
    return Response.json({ accepted: true, callId: result.callId });
  } catch (error) {
    return Response.json({ error: grpcErrorMessage(error) }, { status: grpcErrorStatus(error) });
  } finally {
    clearTimeout(timeout);
  }
}
