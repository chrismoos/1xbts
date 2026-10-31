import { AdmissionEvent } from "@/lib/proto/events/v1/msc";
import { latestAdmissionForMobile, type AdmissionVerdict } from "@/lib/admission";
import { getMscManagementClient, getNetworkManagementClient, grpcErrorStatus } from "@/lib/grpc/client";
import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";
import type { MobileInfo } from "@/lib/proto/bsc/v1/service";
import { AccessTechnology, type PacketSessionInfo } from "@/lib/proto/packet/v1/service";

export const dynamic = "force-dynamic";

const ESN_MANUFACTURER_SHIFT = 24;
const PSEUDO_ESN_MANUFACTURER = 0x80;

interface MobileView {
  address: string;
  pageAddress: string;
  state: string;
  admission?: AdmissionVerdict;
  baseStation: string;
  baseStations: string[];
  mobPRev: number;
  imsi?: string;
  esn?: number;
  meid?: string;
  pgslot?: number;
  slotCycleIndex: number;
  lastHeardMs?: number;
  phoneNumber?: string;
  subscriberId?: string;
  subscriberDisplayName?: string;
  trafficWalshCode?: number;
  trafficServiceOption?: number;
  snrDb?: number;
  signalPowerDb?: number;
  demodQualityPct?: number;
  rxPowerDbm?: number;
  rxLevelDbfs?: number;
  voiceCallState?: string;
  voicePrivacy?: string;
}

export async function GET() {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const stations = await getMscManagementClient().listBaseStations({}, { signal: abort.signal });
    const results = await Promise.allSettled(stations.nodes.map(async (station) => {
      const baseStation = station.managementEndpoint;
      const result = await getNetworkManagementClient().listMobiles(
        { selector: { baseStation } },
        { signal: abort.signal },
      );
      return result.mobiles.map((mobile) => toMobileView(mobile, baseStation));
    }));
    const mobiles = deduplicateMobiles(results.flatMap((result) =>
      result.status === "fulfilled" ? result.value : [],
    ));
    await enrichPacketSessions(mobiles, abort.signal);
    await enrichSubscribers(mobiles, abort.signal);
    try {
      const snapshot = await getMscManagementClient().listAdmissionEvents(
        { imsis: [], meid: "", limit: 0, currentOnly: true },
        { signal: abort.signal },
      );
      if (snapshot.currentSnapshot) {
        const admissions = snapshot.events.map((event) => AdmissionEvent.toJSON(event) as AdmissionVerdict);
        for (const mobile of mobiles) {
          mobile.admission = latestAdmissionForMobile(admissions, mobile);
        }
      }
    } catch {
      // Missing current state must not fall back to an older activity event.
    }
    console.log(`[mobiles] ok (${mobiles.length} mobiles)`);
    return Response.json(mobiles);
  } catch (error) {
    const message = error instanceof Error ? error.message : "unknown error";
    console.log(`[mobiles] aggregation error: ${message}`);
    return Response.json([]);
  } finally {
    clearTimeout(timeout);
  }
}

function toMobileView(mobile: MobileInfo, baseStation: string): MobileView {
  return {
    ...mobile,
    address: mobile.address,
    pageAddress: mobile.pageAddress || mobile.imsi || mobile.address,
    state: mobile.state,
    baseStation,
    baseStations: [baseStation],
    mobPRev: mobile.mobPRev ?? 0,
    imsi: mobile.imsi,
    esn: mobile.esn,
    meid: mobile.meid,
    pgslot: mobile.pgslot,
    slotCycleIndex: mobile.slotCycleIndex ?? 0,
  };
}

function deduplicateMobiles(mobiles: MobileView[]): MobileView[] {
  const deduplicated = new Map<string, MobileView>();
  for (const mobile of mobiles) {
    const key = mobile.meid
      ? `meid:${mobile.meid.toUpperCase()}`
      : mobile.esn != null && (mobile.esn >>> ESN_MANUFACTURER_SHIFT) !== PSEUDO_ESN_MANUFACTURER
        ? `esn:${mobile.esn}`
        : mobile.imsi
          ? `imsi:${mobile.imsi}/esn:${mobile.esn ?? ""}`
          : `${mobile.baseStation}:${mobile.address}`;
    const existing = deduplicated.get(key);
    if (!existing) {
      deduplicated.set(key, mobile);
      continue;
    }
    const baseStations = Array.from(new Set([...existing.baseStations, ...mobile.baseStations]));
    const latest = (mobile.lastHeardMs ?? 0) > (existing.lastHeardMs ?? 0) ? mobile : existing;
    deduplicated.set(key, { ...latest, baseStations });
  }
  return Array.from(deduplicated.values()).sort(
    (left, right) => (right.lastHeardMs ?? 0) - (left.lastHeardMs ?? 0),
  );
}

async function enrichSubscribers(mobiles: MobileView[], signal: AbortSignal) {
  try {
    await waitForHlrReady();
    const client = getHlrClient();
    await Promise.all(
      mobiles.map(async (mobile) => {
        if (!mobile.imsi && mobile.esn == null && !mobile.meid) return;
        try {
          const result = await client.resolveSubscriberByIdentity(
            { identity: { imsi: mobile.imsi, esn: mobile.esn, meid: mobile.meid } },
            { signal },
          );
          mobile.phoneNumber = result.subscriber?.phoneNumber || undefined;
          mobile.subscriberId = result.subscriber?.subscriberId || undefined;
          mobile.subscriberDisplayName = result.subscriber?.displayName || undefined;
        } catch (error) {
          if (grpcErrorStatus(error) === 404) {
            mobile.phoneNumber = undefined;
            mobile.subscriberId = undefined;
            mobile.subscriberDisplayName = undefined;
          }
          return;
        }
      }),
    );
  } catch (error) {
    const message = error instanceof Error ? error.message : "unknown error";
    console.log(`[mobiles] HLR enrichment skipped: ${message}`);
  }
}

async function enrichPacketSessions(mobiles: MobileView[], signal: AbortSignal) {
  try {
    const result = await getNetworkManagementClient().listPcfSessions({}, { signal });
    for (const session of result.sessions) {
      if (
        session.accessTechnology !== AccessTechnology.ACCESS_TECHNOLOGY_HRPD ||
        session.phase === "closed" ||
        mobiles.some((mobile) => mobileMatchesPacketSession(mobile, session)) ||
        !session.subscriberId
      ) {
        continue;
      }
      mobiles.push({
        address: `hrpd-subscriber:${session.subscriberId}`,
        pageAddress: session.mobileAddress || session.sessionId,
        state: session.phase === "active" ? "HRPD Active" : "HRPD",
        baseStation: "hrpd-an",
        baseStations: ["hrpd-an"],
        mobPRev: 0,
        imsi: session.subscriberImsi || session.imsi || undefined,
        esn: session.esn || undefined,
        meid: session.meid || undefined,
        slotCycleIndex: 0,
        lastHeardMs: session.lastActivityAtMs || session.createdAtMs || undefined,
        phoneNumber: session.phoneNumber || undefined,
        subscriberId: session.subscriberId || undefined,
        subscriberDisplayName: session.phoneNumber || session.subscriberId,
        trafficWalshCode: session.trafficWalshCode || undefined,
        trafficServiceOption: session.serviceOption || undefined,
      });
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : "unknown error";
    console.log(`[mobiles] packet-session enrichment skipped: ${message}`);
  }
}

function mobileMatchesPacketSession(mobile: MobileView, session: PacketSessionInfo): boolean {
  if (mobile.subscriberId && session.subscriberId === mobile.subscriberId) return true;
  if (mobile.imsi && (session.subscriberImsi === mobile.imsi || session.imsi === mobile.imsi)) return true;
  if (mobile.esn != null && session.esn === mobile.esn) return true;
  if (mobile.meid && session.meid === mobile.meid) return true;
  return session.mobileAddress === mobile.address;
}
