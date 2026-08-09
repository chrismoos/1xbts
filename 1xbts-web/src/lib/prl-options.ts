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
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_INDICATOR_ON, label: "Roaming (0)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_INDICATOR_OFF, label: "Home (1)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_INDICATOR_FLASHING, label: "Roaming (Flashing) (2)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_OUT_OF_NEIGHBORHOOD, label: "Out of Neighborhood (3)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_OUT_OF_BUILDING, label: "Out of Building (4)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_PREFERRED_SYSTEM, label: "Roaming - Preferred System (5)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_AVAILABLE_SYSTEM, label: "Roaming - Available System (6)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_ALLIANCE_PARTNER, label: "Roaming - Alliance Partner (7)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_PREMIUM_PARTNER, label: "Roaming - Premium Partner (8)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_FULL_SERVICE, label: "Roaming - Full Service Functionality (9)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_PARTIAL_SERVICE, label: "Roaming - Partial Service Functionality (10)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_BANNER_ON, label: "Roaming Banner On (11)" },
  { value: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_BANNER_OFF, label: "Roaming Banner Off (12)" },
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
  { value: 0x01, label: "Cellular analog" },
  { value: 0x02, label: "Cellular CDMA — standard" },
  { value: 0x03, label: "Cellular CDMA — channels" },
  { value: 0x04, label: "Cellular CDMA — preferred" },
  { value: 0x05, label: "PCS — blocks" },
  { value: 0x06, label: "PCS — channels" },
  { value: 0x07, label: "JTACS — standard" },
  { value: 0x08, label: "JTACS — channels" },
  { value: 0x09, label: "Band class 6 — channels" },
];

export const EXTENDED_ACQ_TYPE_OPTIONS: PrlOption<number>[] = [
  ...CLASSIC_ACQ_TYPE_OPTIONS,
  { value: 0x0a, label: "Generic 1x" },
  { value: 0x0b, label: "HRPD" },
  { value: 0x0f, label: "UMB acquisition table" },
  { value: 0x10, label: "UMB" },
];

export function formatRoamingIndicator(raw: number): string {
  switch (raw) {
    case 0: return "Roaming (0)";
    case 1: return "Home (1)";
    case 2: return "Roaming (Flashing) (2)";
    case 3: return "Out of Neighborhood (3)";
    case 4: return "Out of Building (4)";
    case 5: return "Roaming - Preferred System (5)";
    case 6: return "Roaming - Available System (6)";
    case 7: return "Roaming - Alliance Partner (7)";
    case 8: return "Roaming - Premium Partner (8)";
    case 9: return "Roaming - Full Service Functionality (9)";
    case 10: return "Roaming - Partial Service Functionality (10)";
    case 11: return "Roaming Banner On (11)";
    case 12: return "Roaming Banner Off (12)";
    default: return `Reserved (${raw})`;
  }
}
