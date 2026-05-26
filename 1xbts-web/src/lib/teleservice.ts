// C.S0015-B teleservice IDs.
export const TELESERVICE_WMT = 0x1002;
export const TELESERVICE_WAP = 0x1004;

export type TeleserviceKind = "text" | "wap-push" | "other";

export function teleserviceKind(id?: number): TeleserviceKind {
  if (id === undefined || id === TELESERVICE_WMT) return "text";
  if (id === TELESERVICE_WAP) return "wap-push";
  return "other";
}

export function teleserviceName(id?: number): string {
  switch (id) {
    case undefined:
    case TELESERVICE_WMT:
      return "Text";
    case TELESERVICE_WAP:
      return "MMS (WAP Push)";
    default:
      return `0x${id.toString(16).toUpperCase().padStart(4, "0")}`;
  }
}
