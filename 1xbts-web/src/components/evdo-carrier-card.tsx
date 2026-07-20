import { Card, Stat } from "@/components/card";
import { EvdoTxMode, type EvdoCarrierConfig } from "@/lib/proto/bsc/v1/service";

function mhz(hz: number): string {
  return `${(hz / 1e6).toFixed(3)} MHz`;
}

function modeLabel(mode: number): string {
  switch (mode) {
    case EvdoTxMode.EVDO_TX_MODE_ADJACENT_COMPOSITE:
      return "Composite with 1x (centered)";
    case EvdoTxMode.EVDO_TX_MODE_DUAL_RF:
      return "Dual RF (1x + EV-DO)";
    case EvdoTxMode.EVDO_TX_MODE_HRPD_ONLY:
      return "EV-DO only";
    default:
      return "—";
  }
}

/// Presentational EV-DO carrier card. Pure (no hooks), so both the server
/// `/config` page and the client `/hrpd` page can render it.
export function EvdoCarrierCard({ evdo }: { evdo?: EvdoCarrierConfig | null }) {
  if (!evdo) return null;
  return (
    <Card title="EV-DO Carrier">
      <Stat label="Channel" value={String(evdo.channel)} />
      <Stat label="Band Class" value={`BC${evdo.bandClass}`} />
      <Stat label="Frequency" value={mhz(evdo.frequencyHz)} />
      <Stat label="Reverse Freq" value={mhz(evdo.reverseFrequencyHz)} />
      <Stat label="Mode" value={modeLabel(evdo.mode)} />
      {evdo.mode === EvdoTxMode.EVDO_TX_MODE_ADJACENT_COMPOSITE && (
        <Stat
          label="Composite Center"
          value={mhz(evdo.compositeCenterFrequencyHz)}
        />
      )}
      <Stat label="Gain (vs 1x)" value={evdo.gain.toFixed(4)} />
      <Stat label="Advertise on 1x" value={evdo.advertiseOn1x ? "Yes" : "No"} />
      <Stat label="Color Code" value={String(evdo.colorCode)} />
      <Stat label="Subnet Mask" value={`/${evdo.subnetMask}`} />
      <Stat label="Sector ID" value={evdo.sectorId} />
    </Card>
  );
}
