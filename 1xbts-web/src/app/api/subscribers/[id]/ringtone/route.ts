import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";
import { RINGTONE_MAX_UPLOAD_BYTES } from "@/lib/validation";

export const dynamic = "force-dynamic";

// RIFF....WAVE container magic.
function looksLikeWav(bytes: Uint8Array): boolean {
  if (bytes.length < 12) return false;
  return (
    bytes[0] === 0x52 &&
    bytes[1] === 0x49 &&
    bytes[2] === 0x46 &&
    bytes[3] === 0x46 &&
    bytes[8] === 0x57 &&
    bytes[9] === 0x41 &&
    bytes[10] === 0x56 &&
    bytes[11] === 0x45
  );
}

function statusFromError(msg: string): number {
  const lower = msg.toLowerCase();
  if (lower.includes("not found")) return 404;
  if (lower.includes("invalid_argument") || lower.includes("invalid argument"))
    return 400;
  return 502;
}

export async function POST(
  request: Request,
  { params }: { params: Promise<{ id: string }> }
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 30_000);

  try {
    const { id } = await params;
    const form = await request.formData();
    const file = form.get("file");
    if (!(file instanceof File))
      return Response.json({ error: "missing 'file' field" }, { status: 400 });

    if (file.size === 0)
      return Response.json({ error: "file is empty" }, { status: 400 });
    if (file.size > RINGTONE_MAX_UPLOAD_BYTES)
      return Response.json(
        {
          error: `file exceeds ${RINGTONE_MAX_UPLOAD_BYTES / (1024 * 1024)} MB limit`,
        },
        { status: 413 }
      );

    const buf = new Uint8Array(await file.arrayBuffer());
    if (!looksLikeWav(buf))
      return Response.json(
        { error: "file is not a RIFF/WAVE WAV" },
        { status: 400 }
      );

    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.setSubscriberRingtone(
      {
        subscriberId: id,
        wavBytes: buf,
        originalFilename: file.name,
      },
      { signal: abort.signal }
    );
    return Response.json(result);
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: statusFromError(msg) });
  } finally {
    clearTimeout(timeout);
  }
}

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ id: string }> }
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { id } = await params;
    await waitForHlrReady();
    const client = getHlrClient();
    await client.clearSubscriberRingtone(
      { subscriberId: id },
      { signal: abort.signal }
    );
    return new Response(null, { status: 204 });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: statusFromError(msg) });
  } finally {
    clearTimeout(timeout);
  }
}
