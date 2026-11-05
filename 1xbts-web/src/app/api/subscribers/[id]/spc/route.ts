// PUT /api/subscribers/[id]/spc
//
// Sets or clears the per-subscriber Service Programming Code used by
// OTASP `*228` Verify SPC. Body: `{ spc: string | null }`. `null` (or
// omitted / empty) clears the override; the subscriber's handset then
// falls back to the IS-95 default "000000".

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
    const raw = typeof body.spc === "string" ? body.spc.trim() : "";
    if (raw && !/^\d{6}$/.test(raw)) {
      return badRequest("The SPC must be exactly 6 digits", "spc");
    }
    const servicePrgrammingCode = raw || undefined;
    await waitForHlrReady();
    const client = getHlrClient();
    await client.setSubscriberSpc(
      {
        subscriberId: id,
        serviceProgrammingCode: servicePrgrammingCode,
      },
      { signal: abort.signal }
    );
    return Response.json({ ok: true });
  } catch (err) {
    return apiError(err, "save the SPC");
  } finally {
    clearTimeout(timeout);
  }
}
