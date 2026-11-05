import { getMscManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { validateImsi } from "@/lib/validation";
import { apiError, badRequest } from "@/lib/api-error";

const MANAGEMENT_TIMEOUT_MS = 5000;

export const dynamic = "force-dynamic";

export async function POST(request: Request, { params }: { params: Promise<{ id: string }> }) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), MANAGEMENT_TIMEOUT_MS);
  try {
    const { id } = await params;
    const body = await request.json().catch(() => null);
    if (!body || typeof body !== "object" || (body.imsi !== undefined && typeof body.imsi !== "string")) {
      return badRequest("The request must be a JSON object with an optional IMSI");
    }
    const imsi: string | undefined = body.imsi;
    if (imsi !== undefined) {
      const validation = validateImsi(imsi);
      if (!validation.ok) return badRequest(validation.error, "imsi");
    }
    await waitForManagementReady();
    const result = await getMscManagementClient().initiateOtasp({ subscriberId: id, imsi }, { signal: abort.signal });
    return Response.json({ accepted: true, callId: result.callId });
  } catch (error) {
    return apiError(error, "start OTASP");
  } finally {
    clearTimeout(timeout);
  }
}
