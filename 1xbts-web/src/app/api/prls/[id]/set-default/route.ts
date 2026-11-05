import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

type Ctx = { params: Promise<{ id: string }> };

export async function POST(_request: Request, ctx: Ctx) {
  const { id } = await ctx.params;
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    await waitForHlrReady();
    const client = getHlrClient();
    await client.setDefaultPrl({ prlId: id }, { signal: abort.signal });
    return Response.json({ ok: true });
  } catch (err) {
    return apiError(err, "make this the default PRL");
  } finally {
    clearTimeout(timeout);
  }
}
