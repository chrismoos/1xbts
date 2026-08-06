// IS-2000 / C.S0017 service options. Keep in sync with the
// `ServiceOption` enum in `proto/events/v1/pdsn.proto`.
export function serviceOptionName(so: number): string {
  switch (so) {
    case 1: return "Voice (8k)";
    case 2: return "Loopback";
    case 3: return "EVRC";
    case 6: return "SMS";
    case 7: return "Data RC1 (SO7)";
    case 17: return "EVRC";
    case 33: return "Data RC3 (SO33)";
    case 68: return "EVRC-B";
    case 70: return "SMS EXT";
    case 73: return "EVRC-NW";
    case 32768: return "QCELP 13K (SO32768)";
    default: return `SO ${so}`;
  }
}
