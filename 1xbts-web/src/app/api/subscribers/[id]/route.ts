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

function withStatusLabel<T extends { subscriber?: Subscriber }>(result: T) {
  if (!result.subscriber) return result;
  return {
    ...result,
    subscriber: {
      ...result.subscriber,
      status: subscriberStatusLabel(result.subscriber.status),
    },
  };
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
    return Response.json(withStatusLabel(result));
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
    const result = await client.updateSubscriber(
      {
        subscriberId: id,
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
    return Response.json(withStatusLabel(result));
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    const status = msg.toLowerCase().includes("not found") ? 404 : 502;
    return Response.json({ error: msg }, { status });
  } finally {
    clearTimeout(timeout);
  }
}
