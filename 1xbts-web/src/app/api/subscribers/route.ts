import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";
import {
  NumberPlan,
  NumberType,
  Subscriber,
} from "@/lib/proto/hlr/v1/service";
import {
  DEFAULT_NUMBER_PLAN,
  DEFAULT_NUMBER_TYPE,
  parseSubscriberStatus,
  subscriberStatusLabel,
} from "@/lib/subscriber-options";
import { validateImsi, validatePhoneNumber } from "@/lib/validation";

function withStatusLabel(subscriber: Subscriber) {
  return { ...subscriber, status: subscriberStatusLabel(subscriber.status) };
}

export const dynamic = "force-dynamic";

function asString(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function coerceNumberType(value: unknown): NumberType {
  if (typeof value === "number" && NumberType[value] !== undefined) {
    return value as NumberType;
  }
  return DEFAULT_NUMBER_TYPE;
}

function coerceNumberPlan(value: unknown): NumberPlan {
  if (typeof value === "number" && NumberPlan[value] !== undefined) {
    return value as NumberPlan;
  }
  return DEFAULT_NUMBER_PLAN;
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
      subscribers: result.subscribers.map(withStatusLabel),
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
    const phoneCheck = validatePhoneNumber(phoneNumber);
    if (!phoneCheck.ok) {
      return Response.json({ error: phoneCheck.error }, { status: 400 });
    }

    const imsi = asString(body.imsi);
    if (imsi) {
      const imsiCheck = validateImsi(imsi);
      if (!imsiCheck.ok) {
        return Response.json({ error: imsiCheck.error }, { status: 400 });
      }
    }

    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.upsertSubscriber(
      {
        phoneNumber,
        displayName: asString(body.displayName),
        status: parseSubscriberStatus(body.status),
        imsi: imsi || undefined,
        esn: body.esn != null ? Number(body.esn) : undefined,
        numberType: coerceNumberType(body.numberType),
        numberPlan: coerceNumberPlan(body.numberPlan),
      },
      { signal: abort.signal }
    );
    return Response.json(
      result.subscriber
        ? { ...result, subscriber: withStatusLabel(result.subscriber) }
        : result
    );
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
