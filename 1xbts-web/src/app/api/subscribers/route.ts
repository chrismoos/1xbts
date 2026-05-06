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

export async function GET() {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.listSubscribers(
      { limit: 100, offset: 0 },
      { signal: abort.signal }
    );
    return Response.json({
      subscribers: result.subscribers,
      total: result.total,
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}

export async function POST(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const body = await request.json();
    const phoneNumber = asString(body.phoneNumber);
    const validationError = validatePhoneNumber(phoneNumber);
    if (validationError) return validationError;

    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.upsertSubscriber(
      {
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
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}

export async function DELETE(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { searchParams } = new URL(request.url);
    const subscriberId = searchParams.get("id");
    if (!subscriberId) {
      return Response.json({ error: "missing id" }, { status: 400 });
    }
    await waitForHlrReady();
    const client = getHlrClient();
    await client.deleteSubscriber(
      { subscriberId },
      { signal: abort.signal }
    );
    return Response.json({ ok: true });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
