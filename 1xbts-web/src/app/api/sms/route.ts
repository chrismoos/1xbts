import { getMscManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

export async function POST(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const body = await request.json();
    console.log(
      `[sms] gRPC call: dest=${body.destinationNumber} imsi=${body.destinationImsi}`,
    );
    await waitForManagementReady();
    const client = getMscManagementClient();
    const result = await client.sendSms(
      {
        originatingNumber: body.originatingNumber || "",
        text: body.text || "",
        destinationNumber: body.destinationNumber,
        destinationImsi: body.destinationImsi,
      },
      { signal: abort.signal }
    );
    console.log(`[sms] ok: accepted=${result.accepted}`);
    return Response.json(result);
  } catch (err) {
    return apiError(err, "send the message", { accepted: false });
  } finally {
    clearTimeout(timeout);
  }
}
