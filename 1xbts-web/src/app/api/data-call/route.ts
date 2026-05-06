import { getPcfManagementClient, waitForBscReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

export async function POST(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const body = await request.json();
    await waitForBscReady();
    const client = getPcfManagementClient();
    const result = await client.initiateDataCall(
      {
        subscriberId: body.subscriberId || "",
        serviceOption: body.serviceOption ?? 33,
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
