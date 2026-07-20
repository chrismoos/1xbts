"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { Card } from "@/components/card";
import { EvdoCarrierCard } from "@/components/evdo-carrier-card";
import {
  GetSessionsResponse,
  sessionStateToJSON,
  type Session,
  type GetUatiAllocationResponse,
} from "@/lib/proto/an/v1/service";
import type { EvdoCarrierConfig } from "@/lib/proto/bsc/v1/service";
import {
  formatHrpdFullUati,
  hrpdSessionMatchesPacket,
  uatiHex,
  uatiHexDigits,
} from "@/lib/hrpd-correlation";
import {
  mobileForPacketSession,
  mobileLabel,
  useMobileDirectory,
} from "@/lib/mobile-directory";

interface SessionsResponse {
  sessions?: Session[];
  error?: string;
}

interface PacketSessionInfo {
  sessionId: string;
  phase: string;
  accessTechnology: string;
  mobileAddress: string;
  subscriberId: string;
  phoneNumber: string;
  imsi: string;
  meid: string;
  hrpdMnId: string;
  hrpdMnIdSource: string;
  subscriberImsi: string;
  esn: number;
  trafficWalshCode: number;
  serviceOption: number;
}

function stateLabel(state: number): string {
  return sessionStateToJSON(state).replace(/^SESSION_STATE_/, "");
}

function negotiatedSummary(s: Session): string {
  const p = s.protocols;
  if (!p) return "—";
  return `phy=${p.physicalLayer} mac=${p.mac} sec=${p.security}`;
}

export default function HrpdPage() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [packetSessions, setPacketSessions] = useState<PacketSessionInfo[]>([]);
  const [allocation, setAllocation] = useState<GetUatiAllocationResponse | null>(
    null,
  );
  const [evdo, setEvdo] = useState<EvdoCarrierConfig | null>(null);
  const [error, setError] = useState<string | null>(null);
  const mobiles = useMobileDirectory();

  // Carrier config is static at runtime; fetch it once.
  useEffect(() => {
    let cancelled = false;
    fetch("/api/bts-config")
      .then((r) => r.json())
      .then((data: { evdo?: EvdoCarrierConfig; error?: string }) => {
        if (cancelled || data.error) return;
        setEvdo(data.evdo ?? null);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      fetch("/api/an-sessions")
        .then((r) => r.json())
        .then((raw: SessionsResponse) => {
          if (cancelled) return;
          if (raw.error) {
            setError(raw.error);
          } else {
            const data = GetSessionsResponse.fromJSON(raw);
            setError(null);
            setSessions(data.sessions ?? []);
          }
        })
        .catch((e) => {
          if (!cancelled) setError(String(e));
        });
      fetch("/api/an-uati-allocation")
        .then((r) => r.json())
        .then((data: GetUatiAllocationResponse & { error?: string }) => {
          if (cancelled || data.error) return;
          setAllocation(data);
        })
        .catch(() => {});
      fetch("/api/packet-sessions")
        .then((r) => r.json())
        .then((data: { sessions?: PacketSessionInfo[]; error?: string }) => {
          if (cancelled || data.error) return;
          setPacketSessions((data.sessions ?? []).filter((s) => s.accessTechnology === "HRPD"));
        })
        .catch(() => {});
    };
    tick();
    const id = setInterval(tick, 2000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  const usedPct =
    allocation && allocation.capacity > 0
      ? Math.min(100, (allocation.inUse / allocation.capacity) * 100)
      : 0;

  return (
    <div className="p-6 space-y-4">
      <h1 className="text-2xl font-semibold">HRPD Sessions</h1>
      {error && <div className="text-accent-red text-sm">AN service: {error}</div>}

      <EvdoCarrierCard evdo={evdo} />

      {allocation && (
        <Card title="UATI Allocation">
          <div className="space-y-2 text-sm">
            <div className="flex flex-wrap gap-x-6 gap-y-1 font-mono">
              <span className="text-muted">
                color <span className="text-primary">{allocation.colorCode}</span>
              </span>
              <span className="text-muted">
                capacity{" "}
                <span className="text-primary">{allocation.capacity}</span>
              </span>
              <span className="text-muted">
                in use{" "}
                <span className="text-accent-amber">{allocation.inUse}</span>
              </span>
              <span className="text-muted">
                available{" "}
                <span className="text-accent-green">{allocation.available}</span>
              </span>
            </div>
            <div className="h-2 w-full rounded bg-surface-raised overflow-hidden">
              <div
                className="h-full bg-accent-indigo transition-all"
                style={{ width: `${usedPct}%` }}
              />
            </div>
          </div>
        </Card>
      )}

      <Card title={`Sessions (${sessions.length})`}>
        <table className="w-full text-sm">
          <thead className="text-left text-muted">
            <tr>
              <th className="px-3 py-2">UATI</th>
              <th className="px-3 py-2">Color</th>
              <th className="px-3 py-2">State</th>
              <th className="px-3 py-2">Packet Data</th>
              <th className="px-3 py-2">Negotiated</th>
            </tr>
          </thead>
          <tbody>
            {sessions.length === 0 && (
              <tr>
                <td className="px-3 py-3 text-muted" colSpan={5}>
                  No sessions
                </td>
              </tr>
            )}
            {sessions.map((s) => {
              const packet = packetSessions.find((candidate) =>
                hrpdSessionMatchesPacket(s, candidate),
              );
              const mobile = packet ? mobileForPacketSession(packet, mobiles) : undefined;
              const label = mobile ? mobileLabel(mobile) : undefined;
              const canonicalUati = formatHrpdFullUati(s.fullUati);
              return (
                <tr key={s.uati} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 font-mono">
                    <Link
                      href={`/hrpd/${uatiHexDigits(s.uati)}`}
                      className="text-accent-blue hover:underline"
                    >
                      {canonicalUati ?? uatiHex(s.uati)}
                    </Link>
                    {canonicalUati && (
                      <div className="text-xs text-muted">key {uatiHex(s.uati)}</div>
                    )}
                  </td>
                  <td className="px-3 py-2">{s.colorCode}</td>
                  <td className="px-3 py-2">{stateLabel(s.state)}</td>
                  <td className="px-3 py-2 text-xs">
                    {packet ? (
                      <div className="space-y-1">
                        <Link
                          href={`/packets/${encodeURIComponent(packet.sessionId)}`}
                          className="font-mono text-accent-green hover:underline"
                        >
                          {packet.phase} · A10 {packet.trafficWalshCode || "-"}
                        </Link>
                        {mobile && label && (
                          <div>
                            <Link
                              href={`/mobiles/${encodeURIComponent(mobile.address)}`}
                              className="font-mono text-accent-cyan hover:underline"
                            >
                              {label.value}
                            </Link>
                          </div>
                        )}
                      </div>
                    ) : (
                      <span className="text-muted">-</span>
                    )}
                  </td>
                  <td className="px-3 py-2 font-mono text-xs text-muted">
                    {negotiatedSummary(s)}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </Card>
    </div>
  );
}
