import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";
import { RINGTONE_MAX_UPLOAD_BYTES } from "@/lib/validation";
import { apiError, badRequest } from "@/lib/api-error";

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
      return badRequest("Choose a WAV file to upload", "file");

    if (file.size === 0)
      return badRequest("The file is empty", "file");
    if (file.size > RINGTONE_MAX_UPLOAD_BYTES)
      return badRequest(`The file is larger than ${RINGTONE_MAX_UPLOAD_BYTES / (1024 * 1024)} MB`, "file", 413);

    const buf = new Uint8Array(await file.arrayBuffer());
    if (!looksLikeWav(buf))
      return badRequest("The file is not a WAV file", "file");

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
    return apiError(err, "upload the ringtone");
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
    return apiError(err, "remove the ringtone");
  } finally {
    clearTimeout(timeout);
  }
}
