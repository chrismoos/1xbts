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
import {
  validateImei,
  validateImsi,
  validateMeid,
  validatePhoneNumber,
} from "@/lib/validation";
import { apiError, badRequest } from "@/lib/api-error";

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
    return apiError(err, "load subscribers");
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
      return badRequest(phoneCheck.error, "phoneNumber");
    }

    const imsi = asString(body.imsi);
    const meid = asString(body.meid).toLowerCase();
    const imei = asString(body.imei);
    const esn = body.esn != null ? Number(body.esn) : undefined;
    if (imsi) {
      const imsiCheck = validateImsi(imsi);
      if (!imsiCheck.ok) {
        return badRequest(imsiCheck.error, "imsi");
      }
    }
    if (meid) {
      const meidCheck = validateMeid(meid);
      if (!meidCheck.ok) {
        return badRequest(meidCheck.error, "meid");
      }
    }
    if (imei) {
      const imeiCheck = validateImei(imei);
      if (!imeiCheck.ok) {
        return badRequest(imeiCheck.error, "imei");
      }
    }
    if ((imsi || esn !== undefined || meid) && (!imsi || (esn === undefined && !meid))) {
      return badRequest("An identity needs an IMSI plus an ESN or MEID", "imsi");
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
        imei: imei || undefined,
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
    return apiError(err, "create the subscriber");
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
      return badRequest("A subscriber ID is required", "id");
    }
    await waitForHlrReady();
    const client = getHlrClient();
    await client.deleteSubscriber(
      { subscriberId },
      { signal: abort.signal }
    );
    return Response.json({ ok: true });
  } catch (err) {
    return apiError(err, "delete the subscriber");
  } finally {
    clearTimeout(timeout);
  }
}
