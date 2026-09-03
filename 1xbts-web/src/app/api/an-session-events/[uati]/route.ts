import { peerRequestFromUrl, selectorFromUrl } from "@/lib/cell";
import { getNetworkManagementClient } from "@/lib/grpc/client";
import { hrpdRelatedUatis } from "@/lib/hrpd-correlation";
import { AnEventRecord } from "@/lib/proto/an/v1/service";

export const dynamic = "force-dynamic";

export async function GET(
  req: Request,
  { params }: { params: Promise<{ uati: string }> },
) {
  const { uati } = await params;
  const uatiNum = Number.parseInt(uati, 16);
  if (!Number.isFinite(uatiNum)) {
    return Response.json({ error: "invalid uati" }, { status: 400 });
  }
  const scope = {
    selector: selectorFromUrl(req.url),
    cell: peerRequestFromUrl(req.url),
  };
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    const client = getNetworkManagementClient();
    const candidates = new Set<number>([uatiNum >>> 0]);
    try {
      const session = await client.getAnSession(
        { ...scope, request: { uati: uatiNum } },
        { signal: abort.signal },
      );
      if (session.session) {
        for (const candidate of hrpdRelatedUatis(session.session)) {
          candidates.add(candidate);
        }
      }
    } catch {
      // A traffic-UATI detail URL may not have a direct AN session record.
    }

    const records: AnEventRecord[] = [];
    for (const candidate of candidates) {
      const result = await client.getAnSessionEvents(
        { ...scope, request: { uati: candidate, limit: 0 } },
        { signal: abort.signal },
      );
      records.push(...result.records);
    }
    records.sort((left, right) => Number(left.receivedMs) - Number(right.receivedMs));
    return Response.json({
      records: dedupeRecords(records).map((record) => AnEventRecord.toJSON(record)),
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}

function dedupeRecords(records: AnEventRecord[]): AnEventRecord[] {
  const seen = new Set<string>();
  const out: AnEventRecord[] = [];
  for (const record of records) {
    const key = recordKey(record);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(record);
  }
  return out;
}

function recordKey(record: AnEventRecord): string {
  if (record.session) {
    return `s:${record.session.timestampNs}:${record.session.uati}:${record.session.reason}`;
  }
  if (record.access) {
    return `a:${record.access.timestampNs}:${record.access.uati}:${record.access.accessSignature}:${record.access.reason}`;
  }
  if (record.traffic) {
    return `t:${record.traffic.timestampNs}:${record.traffic.uati}:${record.traffic.macIndex}:${record.traffic.reason}:${record.traffic.drcValue}`;
  }
  return `empty:${record.receivedMs}`;
}
