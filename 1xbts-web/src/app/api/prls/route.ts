import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";
import { apiError, badRequest } from "@/lib/api-error";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    const { searchParams } = new URL(request.url);
    const limit = Number(searchParams.get("limit") ?? "100");
    const offset = Number(searchParams.get("offset") ?? "0");
    const prListIdRaw = searchParams.get("prListId");
    const ssprRaw = searchParams.get("ssprPRev");
    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.listPrls(
      {
        limit,
        offset,
        prListId: prListIdRaw != null ? Number(prListIdRaw) : undefined,
        ssprPRev: ssprRaw != null ? Number(ssprRaw) : undefined,
      },
      { signal: abort.signal }
    );
    return Response.json({
      prls: result.prls,
      total: result.total,
    });
  } catch (err) {
    return apiError(err, "load PRLs");
  } finally {
    clearTimeout(timeout);
  }
}

export async function POST(request: Request) {
  // Operator either uploads a .prl file (`rawBytesBase64`) or saves a
  // structured tree built in the editor (`built: PrlDecoded`). The
  // server-side gRPC handler decodes/encodes accordingly via cdma-otasp.
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 10_000);
  try {
    const body = await request.json();
    const name = typeof body.name === "string" ? body.name.trim() : "";
    const notes = typeof body.notes === "string" ? body.notes : "";
    const rawBase64 =
      typeof body.rawBytesBase64 === "string" ? body.rawBytesBase64 : "";
    const built = body.built ?? null;
    if (!name) {
      return badRequest("A PRL name is required", "name");
    }
    if (!rawBase64 && !built) {
      return badRequest("A PRL file or a built PRL is required");
    }
    if (rawBase64 && built) {
      return badRequest("Send a PRL file or a built PRL, not both");
    }
    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.createPrl(
      rawBase64
        ? { name, notes, rawBytes: Buffer.from(rawBase64, "base64") }
        : { name, notes, built },
      { signal: abort.signal }
    );
    return Response.json({ prl: result.prl });
  } catch (err) {
    return apiError(err, "create the PRL");
  } finally {
    clearTimeout(timeout);
  }
}
