// Canonical {value, label} options for every PRL enum the editor uses.

import {
  PrlAbSelection,
  PrlNidInclusion,
  PrlPcsBlock,
  PrlPrefNeg,
  PrlPriority,
  PrlStandardChannel,
  PrlExtSysRecordType,
  PrlRoamingIndicatorKind,
} from "@/lib/proto/hlr/v1/service";

export interface PrlOption<T> {
  value: T;
  label: string;
}

export const AB_OPTIONS: PrlOption<PrlAbSelection>[] = [
  { value: PrlAbSelection.PRL_AB_SELECTION_SYSTEM_A, label: "System A" },
  { value: PrlAbSelection.PRL_AB_SELECTION_SYSTEM_B, label: "System B" },
  { value: PrlAbSelection.PRL_AB_SELECTION_RESERVED, label: "Reserved (10)" },
  { value: PrlAbSelection.PRL_AB_SELECTION_EITHER, label: "System A or B" },
];

export const STD_CHAN_OPTIONS: PrlOption<PrlStandardChannel>[] = [
  { value: PrlStandardChannel.PRL_STANDARD_CHANNEL_RESERVED, label: "Reserved (00)" },
  { value: PrlStandardChannel.PRL_STANDARD_CHANNEL_PRIMARY, label: "Primary" },
  { value: PrlStandardChannel.PRL_STANDARD_CHANNEL_SECONDARY, label: "Secondary" },
  { value: PrlStandardChannel.PRL_STANDARD_CHANNEL_PRIMARY_OR_SECONDARY, label: "Primary or Secondary" },
];

export const PCS_BLOCK_OPTIONS: PrlOption<PrlPcsBlock>[] = [
  { value: PrlPcsBlock.PRL_PCS_BLOCK_A, label: "Block A" },
  { value: PrlPcsBlock.PRL_PCS_BLOCK_B, label: "Block B" },
  { value: PrlPcsBlock.PRL_PCS_BLOCK_C, label: "Block C" },
  { value: PrlPcsBlock.PRL_PCS_BLOCK_D, label: "Block D" },
  { value: PrlPcsBlock.PRL_PCS_BLOCK_E, label: "Block E" },
  { value: PrlPcsBlock.PRL_PCS_BLOCK_F, label: "Block F" },
  { value: PrlPcsBlock.PRL_PCS_BLOCK_RESERVED, label: "Reserved (110)" },
  { value: PrlPcsBlock.PRL_PCS_BLOCK_ANY, label: "Any Block" },
];

export const NID_INCL_OPTIONS: PrlOption<PrlNidInclusion>[] = [
  { value: PrlNidInclusion.PRL_NID_INCLUSION_ANY, label: "Any NID (wildcard 0xFFFF)" },
  { value: PrlNidInclusion.PRL_NID_INCLUSION_SINGLE, label: "Single NID (explicit)" },
  { value: PrlNidInclusion.PRL_NID_INCLUSION_PUBLIC, label: "Public NID (0x0000)" },
  { value: PrlNidInclusion.PRL_NID_INCLUSION_RESERVED, label: "Reserved (11)" },
];

export const PREF_NEG_OPTIONS: PrlOption<PrlPrefNeg>[] = [
  { value: PrlPrefNeg.PRL_PREF_NEG_PREFERRED, label: "Preferred" },
  { value: PrlPrefNeg.PRL_PREF_NEG_NEGATIVE, label: "Negative (do not operate)" },
];

export const PRIORITY_OPTIONS: PrlOption<PrlPriority>[] = [
  { value: PrlPriority.PRL_PRIORITY_MORE_DESIRABLE, label: "More desirable than next" },
  { value: PrlPriority.PRL_PRIORITY_EQUALLY_DESIRABLE, label: "Equally desirable" },
];

export const EXT_SYS_TYPE_OPTIONS: PrlOption<PrlExtSysRecordType>[] = [
  { value: PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_CDMA2000, label: "cdma2000 / IS-95 (0000)" },
  { value: PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_HRPD, label: "HRPD (0001)" },
  { value: PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_MCC_MNC, label: "MCC-MNC (0011)" },
];

export const ROAM_KIND_OPTIONS: PrlOption<PrlRoamingIndicatorKind>[] = [
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_ON_HOME, label: "On Home (0)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_ROAMING, label: "Roaming (1)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_INTERNATIONAL, label: "International (2)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_LTE, label: "LTE (3)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_FLASHING, label: "Flashing (4)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_OTHER, label: "Other (raw byte)" },
];

export interface MccMncSubtypeOption {
  value: "subtype000" | "subtype001" | "subtype010" | "subtype011";
  label: string;
  description: string;
}
export const MCC_MNC_SUBTYPE_OPTIONS: MccMncSubtypeOption[] = [
  { value: "subtype000", label: "MCC + MNC only", description: "Just the country + network code" },
  { value: "subtype001", label: "MCC + MNC + SIDs", description: "Plus a list of 16-bit SIDs" },
  { value: "subtype010", label: "MCC + MNC + SID/NID pairs", description: "Plus a list of (SID, NID) pairs" },
  { value: "subtype011", label: "MCC + MNC + HRPD subnets", description: "Plus a list of HRPD subnet IDs" },
];

export const CLASSIC_ACQ_TYPE_OPTIONS: PrlOption<number>[] = [
  { value: 0x01, label: "Cellular Analog (0001)" },
  { value: 0x02, label: "Cellular CDMA Standard (0010)" },
  { value: 0x03, label: "Cellular CDMA Custom (0011)" },
  { value: 0x04, label: "Cellular CDMA Preferred (0100)" },
  { value: 0x05, label: "PCS CDMA Using Blocks (0101)" },
  { value: 0x06, label: "PCS CDMA Using Channels (0110)" },
  { value: 0x07, label: "JTACS Standard (0111)" },
  { value: 0x08, label: "JTACS Custom (1000)" },
  { value: 0x09, label: "2 GHz Band Class 6 (1001)" },
];

export const EXTENDED_ACQ_TYPE_OPTIONS: PrlOption<number>[] = [
  ...CLASSIC_ACQ_TYPE_OPTIONS,
  { value: 0x0a, label: "Generic 1x / IS-95 (00001010)" },
  { value: 0x0b, label: "Generic HRPD (00001011)" },
  { value: 0x0f, label: "UMB Common Acquisition Table (00001111)" },
  { value: 0x10, label: "Generic UMB (00010000)" },
];

export function formatRoamingIndicator(raw: number): string {
  switch (raw) {
    case 0: return "On Home (0)";
    case 1: return "Roaming (1)";
    case 2: return "International (2)";
    case 3: return "LTE (3)";
    case 4: return "Flashing (4)";
    default: return `Branded (${raw})`;
  }
}
