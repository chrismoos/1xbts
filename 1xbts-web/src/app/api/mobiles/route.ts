import { AdmissionEvent } from "@/lib/proto/events/v1/msc";
import { latestAdmissionForMobile, type AdmissionVerdict } from "@/lib/admission";
import { getMscManagementClient, getNetworkManagementClient, grpcErrorStatus } from "@/lib/grpc/client";
import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";
import type { MobileInfo } from "@/lib/proto/bsc/v1/service";
import { AccessTechnology, type PacketSessionInfo } from "@/lib/proto/packet/v1/service";
import { SessionState, type Session } from "@/lib/proto/an/v1/service";
import type { A10Session } from "@/lib/proto/pcf/v1/service";
import { evdoCells } from "@/lib/grpc/an-aggregate";
import {
  a10ForEvdo,
  evdoIdentity,
  evdoSummary,
  isEvdoSessionOpen,
  packetSessionForEvdo,
  type EvdoPacketSession,
  type EvdoSummary,
} from "@/lib/evdo";
import { apiError } from "@/lib/api-error";

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
  evdo?: EvdoSummary;
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
    const packetSessions = await listPacketSessions(abort.signal);
    const coveredPacketSessions = enrichEvdoSessions(
      mobiles,
      await listAnSessions(abort.signal),
      packetSessions,
      await listA10Sessions(abort.signal),
    );
    enrichPacketSessions(mobiles, packetSessions, coveredPacketSessions);
    mobiles.sort((left, right) => (right.lastHeardMs ?? 0) - (left.lastHeardMs ?? 0));
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
    return apiError(error, "load mobiles");
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

async function listPacketSessions(signal: AbortSignal): Promise<PacketSessionInfo[]> {
  try {
    return (await getNetworkManagementClient().listPcfSessions({}, { signal })).sessions;
  } catch (error) {
    const message = error instanceof Error ? error.message : "unknown error";
    console.log(`[mobiles] packet-session enrichment skipped: ${message}`);
    return [];
  }
}

async function listAnSessions(signal: AbortSignal): Promise<Session[]> {
  const client = getNetworkManagementClient();
  const sessions: Session[] = [];
  try {
    for (const ref of await evdoCells(signal)) {
      try {
        const result = await client.getAnSessions(
          { ...ref, request: { stateFilter: SessionState.SESSION_STATE_UNSPECIFIED } },
          { signal },
        );
        sessions.push(...(result.sessions ?? []));
      } catch {
        // Skip an EV-DO cell whose AN is not answering.
      }
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : "unknown error";
    console.log(`[mobiles] EV-DO session enrichment skipped: ${message}`);
  }
  return sessions;
}

async function listA10Sessions(signal: AbortSignal): Promise<A10Session[]> {
  try {
    return (await getNetworkManagementClient().listPcfA10Sessions({}, { signal })).sessions;
  } catch (error) {
    const message = error instanceof Error ? error.message : "unknown error";
    console.log(`[mobiles] PCF A10 sessions unavailable: ${message}`);
    return [];
  }
}

function evdoPacketSession(session: PacketSessionInfo): EvdoPacketSession {
  return {
    sessionId: session.sessionId,
    phase: session.phase,
    accessTechnology: "HRPD",
    mobileAddress: session.mobileAddress,
    trafficWalshCode: session.trafficWalshCode,
    subscriberId: session.subscriberId,
    subscriberImsi: session.subscriberImsi,
    imsi: session.imsi,
    meid: session.meid,
    esn: session.esn,
    mnId: session.mnId,
    pcfSessionId: session.pcfSessionId,
    mnSessionReferenceId: session.mnSessionReferenceId,
    createdAtMs: session.createdAtMs,
  };
}

function enrichEvdoSessions(
  mobiles: MobileView[],
  anSessions: Session[],
  packetSessions: PacketSessionInfo[],
  a10Sessions: A10Session[],
): Set<string> {
  const evdoPackets = packetSessions.map(evdoPacketSession);
  const covered = new Set<string>();
  for (const session of anSessions) {
    if (!isEvdoSessionOpen(session)) continue;
    const packet = packetSessionForEvdo(session, evdoPackets, a10Sessions);
    if (packet) covered.add(packet.sessionId);
    const identity = evdoIdentity(session, mobiles, packet, a10ForEvdo(session, a10Sessions));
    const summary = evdoSummary(session, packet);
    const matched = identity.mobile as MobileView | undefined;
    if (matched) {
      if (!matched.evdo || (summary.lastActivityAtMs ?? 0) >= (matched.evdo.lastActivityAtMs ?? 0)) {
        matched.evdo = summary;
      }
      continue;
    }
    const hardwareId = identity.hardwareId;
    const key = hardwareId?.meid
      ? `meid:${hardwareId.meid}`
      : hardwareId?.esn != null
        ? `esn:${hardwareId.esn.toString(16).padStart(8, "0")}`
        : identity.subscriberId
          ? `subscriber:${identity.subscriberId}`
          : identity.imsi
            ? `imsi:${identity.imsi}`
            : undefined;
    if (!key) continue;
    mobiles.push({
      address: `evdo-${key}`,
      pageAddress: packet?.mobileAddress || packet?.sessionId || `evdo-${key}`,
      state: session.connected ? "EV-DO Active" : "EV-DO",
      baseStation: "evdo-an",
      baseStations: ["evdo-an"],
      mobPRev: 0,
      imsi: identity.imsi,
      esn: hardwareId?.esn,
      meid: hardwareId?.meid,
      slotCycleIndex: 0,
      lastHeardMs: summary.lastActivityAtMs,
      subscriberId: identity.subscriberId,
      evdo: summary,
    });
  }
  return covered;
}

function enrichPacketSessions(
  mobiles: MobileView[],
  packetSessions: PacketSessionInfo[],
  covered: Set<string>,
) {
  for (const session of packetSessions) {
    if (
      session.accessTechnology !== AccessTechnology.ACCESS_TECHNOLOGY_HRPD ||
      session.phase === "closed" ||
      covered.has(session.sessionId) ||
      mobiles.some((mobile) => mobileMatchesPacketSession(mobile, session)) ||
      !session.subscriberId
    ) {
      continue;
    }
    mobiles.push({
      address: `evdo-subscriber:${session.subscriberId}`,
      pageAddress: session.mobileAddress || session.sessionId,
      state: session.phase === "active" ? "EV-DO Active" : "EV-DO",
      baseStation: "evdo-an",
      baseStations: ["evdo-an"],
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
}

function mobileMatchesPacketSession(mobile: MobileView, session: PacketSessionInfo): boolean {
  if (mobile.subscriberId && session.subscriberId === mobile.subscriberId) return true;
  if (mobile.imsi && (session.subscriberImsi === mobile.imsi || session.imsi === mobile.imsi)) return true;
  if (mobile.esn != null && session.esn === mobile.esn) return true;
  if (mobile.meid && session.meid === mobile.meid) return true;
  return session.mobileAddress === mobile.address;
}
