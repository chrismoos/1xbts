// IS-2000 / C.S0017 service options. Keep in sync with the
// `ServiceOption` enum in `proto/events/v1/pdsn.proto`.
export function serviceOptionName(so: number): string {
  switch (so) {
    case 1: return "Voice (8k)";
    case 3: return "EVRC";
    case 6: return "SMS";
    case 7: return "Data RC1 (SO7)";
    case 33: return "Data RC3 (SO33)";
    case 68: return "EVRC-B";
    case 70: return "SMS EXT";
    default: return `SO ${so}`;
  }
}
