import { mobileForPacketSession, type MobileDirectoryEntry, type PacketSessionDirectoryFields } from "@/lib/mobile-directory";
import { serviceOptionName } from "@/lib/service-option";

export interface PacketBearerFields extends PacketSessionDirectoryFields {
  sessionId: string;
  serviceOption: number;
  pcfSessionId?: number;
}

export interface PacketBearer {
  label: string;
  title: string;
  detail?: string;
  className: string;
}

// An A11-signaled session is EV-DO only when an AN session links to it. A
// femtocell 1x PCF also signals over A11, so the session itself does not say.
export function packetBearer(session: PacketBearerFields, evdoLinked: boolean): PacketBearer {
  if (evdoLinked) {
    return {
      label: "EV-DO",
      title: "EV-DO data session",
      detail: `A10 key ${session.pcfSessionId}`,
      className: "text-accent-purple bg-accent-purple-bg",
    };
  }
  if (session.pcfSessionId != null) {
    return {
      label: "A10",
      title: "A10 data session",
      detail: `A10 key ${session.pcfSessionId}`,
      className: "text-accent-cyan bg-accent-blue-bg",
    };
  }
  return {
    label: "1x",
    title: "1x data session",
    detail: [serviceOptionName(session.serviceOption), session.trafficWalshCode ? `W${session.trafficWalshCode}` : null]
      .filter(Boolean)
      .join(" · "),
    className: "text-accent-blue bg-accent-blue-bg",
  };
}

export function packetSubscriber(
  session: PacketBearerFields,
  mobiles: MobileDirectoryEntry[],
): { mobile?: MobileDirectoryEntry; evdoLinked: boolean } {
  const viaEvdo = mobiles.find((mobile) => mobile.evdo?.packetSessionId === session.sessionId);
  if (viaEvdo) return { mobile: viaEvdo, evdoLinked: true };
  const direct = mobileForPacketSession(session, mobiles);
  if (direct) return { mobile: direct, evdoLinked: false };
  const viaMnId = session.mnId ? mobiles.find((mobile) => mobile.imsi === session.mnId) : undefined;
  return { mobile: viaMnId, evdoLinked: false };
}
