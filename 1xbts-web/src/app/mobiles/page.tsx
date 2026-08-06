"use client";

import { useEffect, useState, useCallback } from "react";
import Link from "next/link";
import { Card } from "@/components/card";
import { esnManufacturer } from "@/lib/esn-manufacturer";
import { formatEsn, formatMeid, formatTimeMs as formatTime } from "@/lib/format";
import { radioConfigPairName } from "@/lib/radio-config";

interface MobileInfo {
  address: string;
  pageAddress: string;
  state: string;
  mobPRev: number;
  esn?: number;
  imsi?: string;
  meid?: string;
  imsiMS1?: number;
  imsiMS2?: number;
  pgslot?: number;
  slotCycleIndex: number;
  snrDb?: number;
  signalPowerDb?: number;
  demodQualityPct?: number;
  rxPowerDbm?: number;
  rxLevelDbfs?: number;
  lastHeardMs?: number;
  phoneNumber?: string;
  subscriberDisplayName?: string;
  subscriberId?: string;
  trafficWalshCode?: number;
  trafficServiceOption?: number;
  trafficPower?: {
    forwardRadioConfig: number;
    reverseRadioConfig: number;
  };
}

function qualityColor(pct: number): string {
  if (pct >= 90) return "text-accent-green";
  if (pct >= 75) return "text-accent-amber";
  return "text-accent-red";
}

function formatMobileLabel(ms: MobileInfo): string {
  const parts: string[] = [];
  if (ms.esn != null) parts.push(`ESN ${formatEsn(ms.esn)}`);
  if (ms.meid) parts.push(`MEID ${formatMeid(ms.meid)}`);
  return parts.length > 0 ? parts.join(" / ") : ms.address;
}

function formatSubscriberId(subscriberId: string): string {
  if (subscriberId.length <= 20) return subscriberId;
  return `${subscriberId.slice(0, 8)}...${subscriberId.slice(-8)}`;
}

export default function MobilesPage() {
  const [mobiles, setMobiles] = useState<MobileInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchMobiles = useCallback(async () => {
    try {
      const res = await fetch("/api/mobiles");
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      setMobiles(data);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "unknown error");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchMobiles();
    const interval = setInterval(fetchMobiles, 3000);
    return () => clearInterval(interval);
  }, [fetchMobiles]);

  return (
    <div className="max-w-7xl mx-auto space-y-6">
      <h1 className="text-lg font-bold">Mobile Stations</h1>

      <Card title={`Registered Mobiles (${mobiles.length})`}>
        {loading ? (
          <p className="text-dimmed text-sm">Loading...</p>
        ) : error ? (
          <p className="text-accent-red text-sm">{error}</p>
        ) : mobiles.length === 0 ? (
          <p className="text-dimmed text-sm">
            No mobile stations registered.
          </p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-muted text-xs">
                  <th className="text-left py-1">Address</th>
                  <th className="text-left py-1">Subscriber</th>
                  <th className="text-left py-1">State</th>
                  <th className="text-left py-1">Traffic</th>
                  <th className="text-right py-1">SNR (dB)</th>
                  <th className="text-right py-1">Rx Level</th>
                  <th className="text-right py-1">Quality</th>
                  <th className="text-left py-1 pl-4">Last Heard</th>
                  <th className="text-left py-1">SCI</th>
                  <th className="text-left py-1"></th>
                </tr>
              </thead>
              <tbody>
                {mobiles.map((ms, i) => {
                  const id = encodeURIComponent(ms.address);
                  const isHrpd = ms.state.startsWith("HRPD");
                  return (
                    <tr key={i} className="border-t border-border hover:bg-hover">
                      <td className="py-2 text-secondary font-mono text-xs">
                        <div>{formatMobileLabel(ms)}</div>
                        <div className="text-[11px] text-muted">
                          <div className="font-mono">
                            IMSI {ms.imsi || "Not Available"}
                          </div>
                          {ms.meid && (
                            <div className="font-mono">
                              MEID {formatMeid(ms.meid)}
                            </div>
                          )}
                          {ms.esn != null && esnManufacturer(ms.esn) && (
                            <span className="text-dimmed">{esnManufacturer(ms.esn)}</span>
                          )}
                        </div>
                      </td>
                      <td className="py-2 text-xs">
                        {ms.subscriberId ? (
                          <div>
                            <Link
                              href={`/subscribers/${encodeURIComponent(ms.subscriberId)}`}
                              className="text-secondary hover:text-accent-green transition-colors"
                            >
                              {ms.subscriberDisplayName || ms.phoneNumber || "Subscriber"}
                            </Link>
                            {ms.subscriberDisplayName && ms.phoneNumber && (
                              <div className="text-muted font-mono text-[11px]">
                                {ms.phoneNumber}
                              </div>
                            )}
                            <div className="text-muted font-mono text-[11px]" title={ms.subscriberId}>
                              {formatSubscriberId(ms.subscriberId)}
                            </div>
                          </div>
                        ) : (
                          <span className="text-dimmed">-</span>
                        )}
                      </td>
                      <td className="py-2">
                        <span
                          className={`text-xs px-2 py-0.5 rounded ${
                            ms.state === "Registered"
                              ? "bg-badge-green-bg text-badge-green-text"
                              : ms.state === "Paged"
                                ? "bg-badge-yellow-bg text-badge-yellow-text"
                                : ms.state === "TrafficAssigning" || ms.state === "TrafficActive"
                                  ? "bg-badge-purple-bg text-badge-purple-text"
                                  : "bg-badge-blue-bg text-badge-blue-text"
                          }`}
                        >
                          {ms.state}
                        </span>
                      </td>
                      <td className="py-2 font-mono text-xs text-secondary">
                        {ms.trafficWalshCode != null ? (
                          <span>
                            {isHrpd ? `A10 ${ms.trafficWalshCode}` : `W${ms.trafficWalshCode}`}
                            {ms.trafficServiceOption != null && (
                              <span className="text-muted ml-1">
                                SO{ms.trafficServiceOption}
                              </span>
                            )}
                            {radioConfigPairName(
                              ms.trafficPower?.forwardRadioConfig,
                              ms.trafficPower?.reverseRadioConfig,
                            ) && (
                              <span className="text-secondary ml-1">
                                {radioConfigPairName(
                                  ms.trafficPower?.forwardRadioConfig,
                                  ms.trafficPower?.reverseRadioConfig,
                                )}
                              </span>
                            )}
                          </span>
                        ) : "-"}
                      </td>
                      <td className="py-2 text-right font-mono text-xs text-secondary">
                        {ms.snrDb != null ? ms.snrDb.toFixed(1) : "-"}
                      </td>
                      <td className="py-2 text-right font-mono text-xs text-secondary">
                        {ms.rxPowerDbm != null
                          ? `${ms.rxPowerDbm.toFixed(1)} dBm`
                          : ms.rxLevelDbfs != null
                          ? `${ms.rxLevelDbfs.toFixed(1)} dBFS`
                          : ms.signalPowerDb != null
                            ? `${ms.signalPowerDb.toFixed(1)} dB`
                            : "-"}
                      </td>
                      <td className="py-2 text-right font-mono text-xs">
                        {ms.demodQualityPct != null ? (
                          <span className={qualityColor(ms.demodQualityPct)}>
                            {ms.demodQualityPct.toFixed(0)}%
                          </span>
                        ) : "-"}
                      </td>
                      <td className="py-2 pl-4 text-muted font-mono text-xs">
                        {ms.lastHeardMs ? formatTime(ms.lastHeardMs) : "-"}
                      </td>
                      <td className="py-2 text-muted text-xs">{ms.slotCycleIndex}</td>
                      <td className="py-2 text-right">
                        <Link
                          href={`/mobiles/${id}`}
                          className="text-xs text-accent-green hover:text-accent-green transition-colors"
                        >
                          View &rarr;
                        </Link>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  );
}
