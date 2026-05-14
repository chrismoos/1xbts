import { getMscManagementClient, waitForBscReady } from "@/lib/grpc/client";
import { validatePhoneNumber } from "@/lib/validation";

export const dynamic = "force-dynamic";

export async function POST(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const body = await request.json();
    const callerNumber =
      typeof body.callerNumber === "string" ? body.callerNumber.trim() : "";
    if (callerNumber) {
      const check = validatePhoneNumber(callerNumber);
      if (!check.ok) {
        return Response.json(
          { accepted: false, message: `callerNumber: ${check.error}` },
          { status: 400 }
        );
      }
    }
    await waitForBscReady();
    const client = getMscManagementClient();
    const result = await client.initiateCall(
      {
        subscriberId: body.subscriberId || "",
        audioFile: body.audioFile || undefined,
        callerNumber: callerNumber || undefined,
      },
      { signal: abort.signal }
    );
    return Response.json(result);
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ accepted: false, message: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
