import { getBtsManagementClient } from "@/lib/grpc/client";
import { Card, Stat } from "@/components/card";

export const dynamic = "force-dynamic";

async function getConfig() {
  try {
    const client = getBtsManagementClient();
    return await client.getBtsConfig({});
  } catch {
    return null;
  }
}

export default async function ConfigPage() {
  const config = await getConfig();

  return (
    <div className="max-w-7xl mx-auto space-y-6">
      <h1 className="text-lg font-bold">System Configuration</h1>

      {!config && (
        <div className="rounded-lg border border-accent-amber/20 bg-accent-amber-bg p-4 text-accent-amber text-sm">
          BSC not reachable.
        </div>
      )}

      {config && (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          <Card title="BTS">
            <Stat label="Pilot Offset" value={String(config.pilotOffset)} />
            <Stat label="Spreading Rate" value={config.spreadingRate} />
            <Stat label="Chip Rate" value={`${config.chipRateHz} Hz`} />
            <Stat label="TX Sample Rate" value={`${config.txSampleRateHz} Hz`} />
            <Stat label="TX Bandwidth" value={`${config.txBandwidthHz} Hz`} />
            <Stat label="Band Class" value={config.bandClass} />
            <Stat label="Band Subclass" value={String(config.bandSubclass)} />
            <Stat label="CDMA Channel" value={String(config.cdmaChannel)} />
            <Stat label="TX Center Freq" value={`${config.txCenterFrequencyHz} Hz`} />
            <Stat label="RX Center Freq" value={`${config.rxCenterFrequencyHz} Hz`} />
            <Stat label="TX Backoff" value={String(config.txDigitalBackoff)} />
            <Stat label="Block Size" value={`${config.blockSizeChips} chips`} />
          </Card>

          <Card title="Pilot Channel">
            {config.pilot && (
              <>
                <Stat label="Walsh Code" value={String(config.pilot.walshCode)} />
                <Stat label="Gain" value={config.pilot.gain.toFixed(4)} />
              </>
            )}
          </Card>

          <Card title="Sync Channel">
            {config.sync && (
              <>
                <Stat label="Walsh Code" value={String(config.sync.walshCode)} />
                <Stat label="Data Rate" value={`${config.sync.dataRateBps} bps`} />
                <Stat label="Gain" value={config.sync.gain.toFixed(4)} />
              </>
            )}
          </Card>

          <Card title="Paging Channel">
            {config.paging && (
              <>
                <Stat label="Walsh Code" value={String(config.paging.walshCode)} />
                <Stat label="Channel #" value={String(config.paging.pagingChannelNumber)} />
                <Stat label="Data Rate" value={`${config.paging.dataRateBps} bps`} />
                <Stat label="Gain" value={config.paging.gain.toFixed(4)} />
              </>
            )}
          </Card>

          <Card title="Overhead Parameters">
            {config.overhead && (
              <>
                <Stat label="SID" value={String(config.overhead.sid)} />
                <Stat label="NID" value={String(config.overhead.nid)} />
                <Stat label="BASE_ID" value={String(config.overhead.baseId)} />
                <Stat label="REG_ZONE" value={String(config.overhead.regZone)} />
                <Stat label="TOTAL_ZONES" value={String(config.overhead.totalZones)} />
                <Stat label="ZONE_TIMER" value={String(config.overhead.zoneTimer)} />
                <Stat label="MAX_SLOT_CYCLE_INDEX" value={String(config.overhead.maxSlotCycleIndex)} />
                <Stat label="PAGE_CHAN" value={String(config.overhead.pageChan)} />
                <Stat label="CONFIG_SEQ" value={String(config.overhead.configSeq)} />
                <Stat label="ACC_CONFIG_SEQ" value={String(config.overhead.accConfigSeq)} />
                <Stat label="POWER_UP_REG" value={config.overhead.powerUpReg ? "Yes" : "No"} />
                <Stat label="PARAMETER_REG" value={config.overhead.parameterReg ? "Yes" : "No"} />
                <Stat label="AUTH_MODE" value={String(config.overhead.authMode)} />
                <Stat label="LP_SEC" value={String(config.overhead.lpSec)} />
                <Stat label="LTM_OFF" value={`${config.overhead.ltmOff} (${formatLtmOff(config.overhead.ltmOff)})`} />
                <Stat label="DAYLT" value={config.overhead.daylt ? "1 (DST)" : "0"} />
              </>
            )}
          </Card>

          <Card title="Timezone">
            {config.timezone && config.timezoneStatus && (
              <>
                <Stat label="Source" value={config.timezone.source} />
                {config.timezoneStatus.tz && (
                  <Stat label="Zone" value={config.timezoneStatus.tz} />
                )}
                <Stat
                  label="LTM_OFF"
                  value={`${config.timezoneStatus.ltmOff} (${formatLtmOff(config.timezoneStatus.ltmOff)})`}
                />
                <Stat
                  label="DAYLT"
                  value={config.timezoneStatus.daylt ? "1 (DST active)" : "0 (no DST)"}
                />
                <Stat label="LP_SEC" value={String(config.timezoneStatus.lpSec)} />
                <Stat
                  label="UTC offset"
                  value={formatUtcOffset(config.timezoneStatus.utcOffsetSeconds)}
                />
              </>
            )}
          </Card>
        </div>
      )}
    </div>
  );
}

function formatLtmOff(halfHours: number): string {
  const minutes = halfHours * 30;
  const sign = minutes < 0 ? "-" : "+";
  const abs = Math.abs(minutes);
  const h = Math.floor(abs / 60);
  const m = abs % 60;
  return `UTC${sign}${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
}

function formatUtcOffset(seconds: number): string {
  const sign = seconds < 0 ? "-" : "+";
  const abs = Math.abs(seconds);
  const h = Math.floor(abs / 3600);
  const m = Math.floor((abs % 3600) / 60);
  return `${sign}${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
}
