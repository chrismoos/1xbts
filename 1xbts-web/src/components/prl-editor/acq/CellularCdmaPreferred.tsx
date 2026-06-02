import {
  PrlAcqRecord,
  PrlExtAcqRecord,
} from "@/lib/proto/hlr/v1/service";
import { AB_OPTIONS } from "@/lib/prl-options";
import { EnumSelect } from "../shared/EnumSelect";

export function CellularCdmaPreferredEditor({
  record,
  onPatch,
}: {
  record: PrlAcqRecord | PrlExtAcqRecord;
  onPatch: (mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void) => void;
}) {
  return (
    <EnumSelect
      label="A/B Selection (Cellular CDMA preferred — CDMA-first, analog fallback)"
      value={record.cellularCdmaPreferred!.ab}
      options={AB_OPTIONS}
      onChange={(next) =>
        onPatch((d) => {
          if (d.cellularCdmaPreferred) d.cellularCdmaPreferred.ab = next;
        })
      }
    />
  );
}
