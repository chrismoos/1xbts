import { getMscManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { validatePhoneNumber } from "@/lib/validation";
import { apiError } from "@/lib/api-error";

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
    const serviceOption =
      body.serviceOption === undefined ? undefined : Number(body.serviceOption);
    if (serviceOption !== undefined && !(Number.isInteger(serviceOption) && serviceOption > 0)) {
      return Response.json(
        { accepted: false, message: "serviceOption must be a positive integer" },
        { status: 400 }
      );
    }
    await waitForManagementReady();
    const client = getMscManagementClient();
    const result = await client.initiateCall(
      {
        subscriberId: body.subscriberId || "",
        audioFile: body.audioFile || undefined,
        callerNumber: callerNumber || undefined,
        serviceOption,
      },
      { signal: abort.signal }
    );
    return Response.json(result);
  } catch (err) {
    return apiError(err, "place the call", { accepted: false });
  } finally {
    clearTimeout(timeout);
  }
}
