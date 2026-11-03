// The outer loop's starting targets in cdma-bts power_control.rs. RC1 and RC2
// read data Eb/Nt on the Walsh-symbol scale and RC3 reads pilot SINR, so the
// two cannot share thresholds.
const NOMINAL_DB: Record<number, number> = { 1: 13, 2: 11, 3: 0 };

const EXCELLENT_MARGIN_DB = 2;
const GOOD_MARGIN_DB = -2;
const AVERAGE_MARGIN_DB = -5;

export function reverseMetricLabel(reverseRc?: number): string {
  return reverseRc === 3 ? "Pilot SINR" : "Eb/Nt";
}

// Power control holds a healthy call near its target, so 3 bars is normal.
export function reverseQualityBars(reverseRc?: number, measuredDb?: number): number | undefined {
  if (reverseRc == null || measuredDb == null || !Number.isFinite(measuredDb)) return undefined;
  const nominal = NOMINAL_DB[reverseRc];
  if (nominal == null) return undefined;
  const margin = measuredDb - nominal;
  if (margin >= EXCELLENT_MARGIN_DB) return 4;
  if (margin >= GOOD_MARGIN_DB) return 3;
  if (margin >= AVERAGE_MARGIN_DB) return 2;
  return 1;
}

export interface ReversePowerReading {
  measuredInnerLoop1sDb?: number;
  lastPcgPilotEcNtDb?: number[];
  lastPcgSnrDb?: number[];
  lastActivePcgMask?: boolean[];
}

export function measuredReverseDb(power?: ReversePowerReading): number | undefined {
  if (!power) return undefined;
  if (power.measuredInnerLoop1sDb != null && Number.isFinite(power.measuredInnerLoop1sDb)) {
    return power.measuredInnerLoop1sDb;
  }
  const pilot = power.lastPcgPilotEcNtDb ?? [];
  const pcgs = pilot.length > 0 ? pilot : (power.lastPcgSnrDb ?? []);
  const mask = power.lastActivePcgMask;
  const values = pcgs.filter(
    (db, i) => Number.isFinite(db) && (!mask || mask.length !== pcgs.length || mask[i]),
  );
  return values.length > 0 ? values.reduce((a, b) => a + b, 0) / values.length : undefined;
}

const SMOOTHING_SAMPLES = 5;

// A median ignores the tens-of-dB jump a probe or a misdecoded frame puts in one reading.
export function recentMedianDb(values: number[]): number | undefined {
  const recent = values.slice(-SMOOTHING_SAMPLES).filter((v) => Number.isFinite(v));
  if (recent.length === 0) return undefined;
  const sorted = [...recent].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}
