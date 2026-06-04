import { grpcErrorMessage, grpcErrorStatus } from "@/lib/grpc/client";
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
import { validateImsi, validateMeid, validatePhoneNumber } from "@/lib/validation";

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
    return Response.json(
      { error: grpcErrorMessage(err) },
      { status: grpcErrorStatus(err) }
    );
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
    const meid = asString(body.meid).toLowerCase();
    const esn = body.esn != null ? Number(body.esn) : undefined;
    if (imsi) {
      const imsiCheck = validateImsi(imsi);
      if (!imsiCheck.ok) {
        return Response.json({ error: imsiCheck.error }, { status: 400 });
      }
    }
    if (meid) {
      const meidCheck = validateMeid(meid);
      if (!meidCheck.ok) {
        return Response.json({ error: meidCheck.error }, { status: 400 });
      }
    }
    if ((imsi || esn !== undefined || meid) && (!imsi || (esn === undefined && !meid))) {
      return Response.json(
        { error: "Subscriber identity requires IMSI plus ESN or MEID" },
        { status: 400 }
      );
    }

    await waitForHlrReady();
    const client = getHlrClient();
    const result = await client.upsertSubscriber(
      {
        phoneNumber,
        displayName: asString(body.displayName),
        status: parseSubscriberStatus(body.status),
        imsi: imsi || undefined,
        esn,
        meid: meid || undefined,
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
    return Response.json(
      { error: grpcErrorMessage(err) },
      { status: grpcErrorStatus(err) }
    );
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
    return Response.json(
      { error: grpcErrorMessage(err) },
      { status: grpcErrorStatus(err) }
    );
  } finally {
    clearTimeout(timeout);
  }
}
