// IMSI = MCC (3) || IMSI_11_12 (2) || IMSI_S (10), per C.S0005-E §2.3.1.
// The 10-digit IMSI_S mirrors the MIN. Mapping from MDN:
//   - ≤ 10 digits: left-pad with zeros to fit
//   - > 10 digits (international / country-code prefixed): take the
//     last 10 digits. The HLR enforces uniqueness on the resulting
//     IMSI; if two MDNs share the same last 10 digits the operator
//     will get a collision at save time and can pick a different
//     scheme.

const IMSI_S_DIGITS = 10;

export function generateImsi(
  phoneNumber: string,
  mcc: string,
  imsi1112: string,
): { imsi: string; error?: string } {
  const phoneDigits = phoneNumber.replace(/\D/g, "");
  if (phoneDigits.length === 0) {
    return { imsi: "", error: "phone number is empty" };
  }
  if (!/^\d{3}$/.test(mcc) || !/^\d{2}$/.test(imsi1112)) {
    return { imsi: "", error: "cell identity not loaded" };
  }
  const min =
    phoneDigits.length > IMSI_S_DIGITS
      ? phoneDigits.slice(-IMSI_S_DIGITS)
      : phoneDigits.padStart(IMSI_S_DIGITS, "0");
  return { imsi: `${mcc}${imsi1112}${min}` };
}
