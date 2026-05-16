// On-air NUMBER_TYPE / NUMBER_PLAN wire values per ANSI T1.607
// (carried in C.S0005-E §3.7.5.3 Calling Party Number records and in
// C.S0005-E §2.7.1.3.2.4 Origination messages).

export function numberTypeName(value: number | null | undefined): string | null {
  if (value == null) return null;
  switch (value) {
    case 0:
      return "Unknown";
    case 1:
      return "International";
    case 2:
      return "National";
    case 3:
      return "Network specific";
    case 4:
      return "Subscriber";
    case 6:
      return "Abbreviated";
    default:
      return null;
  }
}

export function numberPlanName(value: number | null | undefined): string | null {
  if (value == null) return null;
  switch (value) {
    case 0:
      return "Unknown";
    case 1:
      return "ISDN / E.164";
    case 3:
      return "Data (X.121)";
    case 4:
      return "Telex (F.69)";
    case 9:
      return "Private";
    default:
      return null;
  }
}

export function formatNumberType(value: number | null | undefined): string | number | null {
  if (value == null) return null;
  const name = numberTypeName(value);
  return name ? `${name} (${value})` : value;
}

export function formatNumberPlan(value: number | null | undefined): string | number | null {
  if (value == null) return null;
  const name = numberPlanName(value);
  return name ? `${name} (${value})` : value;
}
