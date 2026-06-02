import {
  PrlAcqRecord,
  PrlExtAcqRecord,
} from "@/lib/proto/hlr/v1/service";
import {
  CLASSIC_ACQ_TYPE_OPTIONS,
  EXTENDED_ACQ_TYPE_OPTIONS,
} from "@/lib/prl-options";

function typeLabelForRaw(raw: number, mode: "classic" | "extended"): string {
  const options =
    mode === "extended" ? EXTENDED_ACQ_TYPE_OPTIONS : CLASSIC_ACQ_TYPE_OPTIONS;
  const hit = options.find((o) => o.value === raw);
  return hit ? hit.label.replace(/\s*\([01]+\)$/, "") : `Type 0x${raw.toString(16)}`;
}

export function acqShortLabel(
  record: PrlAcqRecord | PrlExtAcqRecord,
  mode: "classic" | "extended",
): string {
  return typeLabelForRaw(record.acqTypeRaw, mode);
}

export function acqRowSummary(
  index: number,
  record: PrlAcqRecord | PrlExtAcqRecord,
  mode: "classic" | "extended",
): string {
  return `#${index} — ${acqShortLabel(record, mode)}`;
}
