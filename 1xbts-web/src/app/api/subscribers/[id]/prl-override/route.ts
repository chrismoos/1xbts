// PUT /api/subscribers/[id]/prl-override
//
// Sets or clears the per-subscriber PRL override. Body: `{ prlId: string | null }`.
// `null` (or omitted prlId) clears the override; the subscriber then falls back
// to the system default PRL during OTASP `*228`.

import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";

export const dynamic = "force-dynamic";

type Ctx = { params: Promise<{ id: string }> };

export async function PUT(request: Request, ctx: Ctx) {
  const { id } = await ctx.params;
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    const body = await request.json().catch(() => ({}));
    const prlId = typeof body.prlId === "string" && body.prlId ? body.prlId : undefined;
    await waitForHlrReady();
    const client = getHlrClient();
    await client.setSubscriberPrlOverride(
      { subscriberId: id, prlId },
      { signal: abort.signal }
    );
    return Response.json({ ok: true });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = /INVALID_ARGUMENT|not found/i.test(msg) ? 400 : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}
