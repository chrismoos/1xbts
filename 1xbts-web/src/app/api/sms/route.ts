import { getMscManagementClient, waitForBscReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

export async function POST(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const body = await request.json();
    console.log(
      `[sms] gRPC call: dest=${body.destinationNumber} imsi=${body.destinationImsi}`,
    );
    await waitForBscReady();
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
    const msg = err instanceof Error ? err.message : "unknown error";
    console.log(`[sms] gRPC error: ${msg}`);
    return Response.json({ accepted: false, message: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
