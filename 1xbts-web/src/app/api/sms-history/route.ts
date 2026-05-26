import { getSmscClient } from "@/lib/grpc/smsc-client";
import type { SmsSubmission } from "@/lib/proto/smsc/v1/service";

export const dynamic = "force-dynamic";

function bytesToHex(b: Uint8Array | undefined): string | undefined {
  if (!b || b.length === 0) return undefined;
  return Array.from(b, x => x.toString(16).padStart(2, "0")).join("");
}

function shape(s: SmsSubmission) {
  return {
    smsId: s.smsId,
    originatingNumber: s.originatingNumber,
    destinationNumber: s.destinationNumber,
    originatingSubscriberId: s.originatingSubscriberId,
    destinationSubscriberId: s.destinationSubscriberId,
    destinationEsn: s.destinationEsn,
    destinationImsi: s.destinationImsi,
    text: s.text,
    state: s.state,
    failureReason: s.failureReason,
    createdAt: s.createdAt,
    updatedAt: s.updatedAt,
    teleserviceId: s.teleserviceId,
    rawUserDataHex: bytesToHex(s.rawUserData),
  };
}

export async function GET(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { searchParams } = new URL(request.url);
    const limit = parseInt(searchParams.get("limit") || "50");
    const offset = parseInt(searchParams.get("offset") || "0");
    const state = searchParams.get("state") || undefined;
    const phone = searchParams.get("phone") || undefined;

    const client = getSmscClient();

    // `phone` is a convenience filter for per-subscriber views: fetch
    // outbound (originating=phone) and inbound (destination=phone)
    // submissions in parallel, dedupe by smsId, sort by createdAt desc.
    if (phone) {
      const [outbound, inbound] = await Promise.all([
        client.listSmsSubmissions(
          { limit, offset, state, originatingNumber: phone },
          { signal: abort.signal },
        ),
        client.listSmsSubmissions(
          { limit, offset, state, destinationNumber: phone },
          { signal: abort.signal },
        ),
      ]);
      const merged = new Map<string, SmsSubmission>();
      for (const s of outbound.submissions) merged.set(s.smsId, s);
      for (const s of inbound.submissions) merged.set(s.smsId, s);
      const submissions = Array.from(merged.values())
        .sort((a, b) => {
          const ta = a.createdAt ? new Date(a.createdAt).getTime() : 0;
          const tb = b.createdAt ? new Date(b.createdAt).getTime() : 0;
          return tb - ta;
        })
        .slice(0, limit)
        .map(shape);
      return Response.json({
        submissions,
        total: submissions.length,
      });
    }

    const result = await client.listSmsSubmissions(
      { limit, offset, state, destinationNumber: undefined },
      { signal: abort.signal },
    );
    return Response.json({
      submissions: result.submissions.map(shape),
      total: result.total,
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
