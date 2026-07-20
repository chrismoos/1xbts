import {
  getBscManagementClient,
  getPcfManagementClient,
  waitForBscReady,
} from "@/lib/grpc/client";
import {
  AccessTechnology,
  type PacketSessionInfo,
} from "@/lib/proto/packet/v1/service";

export const dynamic = "force-dynamic";

export async function GET() {
  const abort = AbortController.prototype
    ? new AbortController()
    : { signal: undefined, abort() {} };
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    console.log("[mobiles] gRPC call");
    await waitForBscReady();
    const bscClient = getBscManagementClient();
    const result = await bscClient.listMobiles({}, { signal: abort.signal });
    const mobiles = [...result.mobiles];

    try {
      const pcfClient = getPcfManagementClient();
      const packetResult = await pcfClient.listPcfSessions({}, { signal: abort.signal });
      for (const session of packetResult.sessions) {
        if (
          session.accessTechnology !== AccessTechnology.ACCESS_TECHNOLOGY_HRPD ||
          session.phase === "closed"
        )
          continue;
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
