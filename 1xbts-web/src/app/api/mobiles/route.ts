import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { cellForColorCode, selectorFromUrl } from "@/lib/cell";
import type { BsSelector } from "@/lib/proto/mgmt/v1/service";
import { hrpdPacketSessionColorCode } from "@/lib/hrpd-correlation";
import {
  AccessTechnology,
  type PacketSessionInfo,
} from "@/lib/proto/packet/v1/service";
import type { BtsSummary } from "@/lib/proto/bts_management/v1/service";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const abort = AbortController.prototype
    ? new AbortController()
    : { signal: undefined, abort() {} };
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    console.log("[mobiles] gRPC call");
    await waitForManagementReady();
    const client = getNetworkManagementClient();
    // A named base station lists its own mobiles. Without one, the network-
    // wide dashboard aggregates every base station.
    const selector = selectorFromUrl(request.url);
    const result = selector.baseStation
      ? await client.listMobiles({ selector }, { signal: abort.signal })
      : await client.listAllMobiles({}, { signal: abort.signal });
    const mobiles = [...result.mobiles];

    try {
      const packetResult = await client.listPcfSessions({}, { signal: abort.signal });
      const cells = packetResult.sessions.some(isOpenHrpdSession)
        ? await listCells(selector, abort.signal)
        : [];
      for (const session of packetResult.sessions) {
        if (!isOpenHrpdSession(session)) continue;
        if (mobiles.some((mobile) => mobileMatchesPacketSession(mobile, session))) continue;
        if (!session.subscriberId) continue;

        const subscriberImsi = session.subscriberImsi || session.imsi || "";
        mobiles.push({
          address: `hrpd-subscriber:${session.subscriberId}`,
          pageAddress: session.mobileAddress || session.sessionId,
          state: session.phase === "active" ? "HRPD Active" : "HRPD",
          mobPRev: 0,
          imsi: subscriberImsi || undefined,
          esn: session.esn || undefined,
          meid: session.meid || undefined,
          pgslot: undefined,
          slotCycleIndex: 0,
          lastHeardMs: session.lastActivityAtMs || session.createdAtMs || undefined,
          phoneNumber: session.phoneNumber || undefined,
          subscriberId: session.subscriberId || undefined,
          subscriberDisplayName: session.phoneNumber || session.subscriberId,
          trafficWalshCode: session.trafficWalshCode || undefined,
          trafficServiceOption: session.serviceOption || undefined,
          voiceCallState: undefined,
          servingCell: cellForColorCode(cells, hrpdPacketSessionColorCode(session)),
        });
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : "unknown error";
      console.log(`[mobiles] packet-session enrichment skipped: ${msg}`);
    }

    console.log(`[mobiles] ok (${mobiles.length} mobiles)`);
    return Response.json(mobiles);
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    console.log(`[mobiles] gRPC error: ${msg}`);
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}

function isOpenHrpdSession(session: PacketSessionInfo): boolean {
  return (
    session.accessTechnology === AccessTechnology.ACCESS_TECHNOLOGY_HRPD &&
    session.phase !== "closed"
  );
}

// Enrolled cells, for attributing an HRPD session to the one that issued its
// UATI. A failed lookup costs the attribution, not the session rows.
async function listCells(
  selector: BsSelector,
  signal?: AbortSignal,
): Promise<BtsSummary[]> {
  try {
    const result = await getNetworkManagementClient().listBts(
      { selector },
      { signal },
    );
    return result.bts;
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    console.log(`[mobiles] cell list unavailable: ${msg}`);
    return [];
  }
}

function mobileMatchesPacketSession(
  mobile: {
    address?: string;
    subscriberId?: string;
    imsi?: string;
    esn?: number;
    meid?: string;
  },
  session: PacketSessionInfo,
): boolean {
  if (mobile.subscriberId && session.subscriberId === mobile.subscriberId) return true;
  if (mobile.imsi && (session.subscriberImsi === mobile.imsi || session.imsi === mobile.imsi)) {
    return true;
  }
  if (mobile.esn != null && session.esn === mobile.esn) return true;
  if (mobile.meid && session.meid === mobile.meid) return true;
  if (mobile.address && session.mobileAddress === mobile.address) return true;
  return false;
}
