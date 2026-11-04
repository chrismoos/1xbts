// PUT /api/subscribers/[id]/firstchp-override
//
// Sets or clears the per-subscriber FIRSTCHP override (analog first
// paging/control channel, 0–2047) written during OTASP `*228` NAM
// download. Body: `{ firstchp: number | null }`. `null` (or omitted)
// clears the override; OTASP then preserves the handset's existing value.

import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";
import { apiError, badRequest } from "@/lib/api-error";

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
        return badRequest("FIRSTCHP must be a whole number from 0 to 2047", "firstchp");
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
    return apiError(err, "save the FIRSTCHP override");
  } finally {
    clearTimeout(timeout);
  }
}
