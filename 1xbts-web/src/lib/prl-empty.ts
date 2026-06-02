// Minimal valid PRLs for "build from scratch" flow.
// The server-side encoder will compute PR_LIST_SIZE and CRC at save time;
// we just need a structurally valid template the editor can render.

import {
  PrlDecoded,
  PrlRoamingIndicatorKind,
} from "@/lib/proto/hlr/v1/service";
import { emptyClassicAcq } from "@/components/prl-editor/builders";

const DEFAULT_ROAM = {
  raw: 0,
  kind: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_ON_HOME,
};

export function emptyClassicPrl(prListId: number): PrlDecoded {
  return {
    classic: {
      prListSize: 0,
      prListId,
      prefOnly: false,
      defRoamInd: DEFAULT_ROAM,
      prListCrc: 0,
      computedCrc: 0,
      crcOk: true,
      // One CellularCdmaPreferred to satisfy "at least one acq record".
      acquisitionRecords: [emptyClassicAcq(0x04)],
      systemRecords: [],
    },
  };
}

export function emptyExtendedPrl(prListId: number): PrlDecoded {
  return {
    extended: {
      prListSize: 0,
      prListId,
      curSsprPRev: 3,
      prefOnly: false,
      defRoamInd: DEFAULT_ROAM,
      prListCrc: 0,
      computedCrc: 0,
      crcOk: true,
      acquisitionRecords: [],
      commonSubnetRecords: [],
      systemRecords: [],
    },
  };
}
