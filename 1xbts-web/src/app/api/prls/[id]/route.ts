import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";

export const dynamic = "force-dynamic";

type Ctx = { params: Promise<{ id: string }> };

export async function GET(_request: Request, ctx: Ctx) {
  const { id } = await ctx.params;
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.getPrl({ prlId: id }, { signal: abort.signal });
    return Response.json({ prl: result.prl });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = /NOT_FOUND/.test(msg) ? 404 : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}

export async function PATCH(request: Request, ctx: Ctx) {
  const { id } = await ctx.params;
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 10_000);
  try {
    const body = await request.json();
    const name = typeof body.name === "string" ? body.name : undefined;
    const notes = typeof body.notes === "string" ? body.notes : undefined;
    const rawBase64 =
      typeof body.rawBytesBase64 === "string" ? body.rawBytesBase64 : undefined;
    const built = body.built ?? undefined;
    if (rawBase64 && built) {
      return Response.json(
        { error: "send rawBytesBase64 OR built, not both" },
        { status: 400 }
      );
    }
    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.updatePrl(
      {
        prlId: id,
        name,
        notes,
        rawBytes: rawBase64 ? Buffer.from(rawBase64, "base64") : undefined,
        built,
      },
      { signal: abort.signal }
    );
    return Response.json({ prl: result.prl });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = /INVALID_ARGUMENT|PRL validation/.test(msg) ? 400 : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}

export async function DELETE(_request: Request, ctx: Ctx) {
  const { id } = await ctx.params;
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    await waitForHlrReady();
    const client = getHlrClient();
    await client.deletePrl({ prlId: id }, { signal: abort.signal });
    return Response.json({ ok: true });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = /FAILED_PRECONDITION/.test(msg) ? 409 : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}
