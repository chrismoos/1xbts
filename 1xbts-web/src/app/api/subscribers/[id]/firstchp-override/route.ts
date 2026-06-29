// PUT /api/subscribers/[id]/firstchp-override
//
// Sets or clears the per-subscriber FIRSTCHP override (analog first
// paging/control channel, 0–2047) written during OTASP `*228` NAM
// download. Body: `{ firstchp: number | null }`. `null` (or omitted)
// clears the override; OTASP then preserves the handset's existing value.

import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";

export const dynamic = "force-dynamic";

type Ctx = { params: Promise<{ id: string }> };

export async function PUT(request: Request, ctx: Ctx) {
  const { id } = await ctx.params;
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    const body = await request.json().catch(() => ({}));
    let firstchpOverride: number | undefined;
    if (body.firstchp !== null && body.firstchp !== undefined) {
      const n = Number(body.firstchp);
      if (!Number.isInteger(n) || n < 0 || n > 2047) {
        return Response.json(
          { error: "FIRSTCHP must be an integer in 0–2047" },
          { status: 400 }
        );
      }
      firstchpOverride = n;
    }
    await waitForHlrReady();
    const client = getHlrClient();
    await client.setSubscriberFirstchpOverride(
      {
        subscriberId: id,
        firstchpOverride,
      },
      { signal: abort.signal }
    );
    return Response.json({ ok: true });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = /INVALID_ARGUMENT|not found|range|0–2047/i.test(msg)
      ? 400
      : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}
