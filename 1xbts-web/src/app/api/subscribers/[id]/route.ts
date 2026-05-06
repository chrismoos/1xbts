import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";

export const dynamic = "force-dynamic";

function asString(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function validatePhoneNumber(phoneNumber: string): Response | null {
  if (!/^\d+$/.test(phoneNumber)) {
    return Response.json(
      { error: "phoneNumber must contain at least one digit and only digits" },
      { status: 400 }
    );
  }
  return null;
}

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ id: string }> }
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { id } = await params;
    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.getSubscriber(
      { subscriberId: id },
      { signal: abort.signal }
    );
    return Response.json(result);
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = msg.toLowerCase().includes("not found") ? 404 : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}

export async function PATCH(
  request: Request,
  { params }: { params: Promise<{ id: string }> }
) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { id } = await params;
    const body = await request.json();
    const phoneNumber = asString(body.phoneNumber);
    const validationError = validatePhoneNumber(phoneNumber);
    if (validationError) return validationError;

    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.updateSubscriber(
      {
        subscriberId: id,
        phoneNumber,
        displayName: asString(body.displayName),
        status: asString(body.status) || "active",
        imsi: asString(body.imsi) || undefined,
        esn: body.esn != null ? Number(body.esn) : undefined,
      },
      { signal: abort.signal }
    );
    return Response.json(result);
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = msg.toLowerCase().includes("not found") ? 404 : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}
