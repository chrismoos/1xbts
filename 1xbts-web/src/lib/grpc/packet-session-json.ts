import {
  AccessTechnology,
  HrpdMnIdSource,
  type PacketSessionInfo,
} from "../proto/packet/v1/service";

// The web UI has always consumed these two fields as lowercase/label strings
// ("1x"/"HRPD", "a11"/"derived_hardware"). The proto now carries them as typed
// enums, so translate back to the legacy labels at the API-route boundary and
// keep the browser contract unchanged.
function accessTechnologyLabel(value: AccessTechnology): string {
  switch (value) {
    case AccessTechnology.ACCESS_TECHNOLOGY_HRPD:
      return "HRPD";
    case AccessTechnology.ACCESS_TECHNOLOGY_CDMA_1X:
      return "1x";
    default:
      return "";
  }
}

function hrpdMnIdSourceLabel(value: HrpdMnIdSource): string {
  switch (value) {
    case HrpdMnIdSource.HRPD_MN_ID_SOURCE_A11:
      return "a11";
    case HrpdMnIdSource.HRPD_MN_ID_SOURCE_DERIVED_HARDWARE:
      return "derived_hardware";
    default:
      return "";
  }
}

export type PacketSessionJson = Omit<
  PacketSessionInfo,
  "accessTechnology" | "hrpdMnIdSource"
> & {
  accessTechnology: string;
  hrpdMnIdSource: string;
};

export function packetSessionToJson(
  session: PacketSessionInfo,
): PacketSessionJson {
  return {
    ...session,
    accessTechnology: accessTechnologyLabel(session.accessTechnology),
    hrpdMnIdSource: hrpdMnIdSourceLabel(session.hrpdMnIdSource),
  };
}
