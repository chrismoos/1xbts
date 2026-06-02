import {
  PrlAcqRecord,
  PrlExtAcqRecord,
} from "@/lib/proto/hlr/v1/service";
import { AB_OPTIONS } from "@/lib/prl-options";
import { EnumSelect } from "../shared/EnumSelect";

export function CellularAnalogEditor({
  record,
  onPatch,
}: {
  record: PrlAcqRecord | PrlExtAcqRecord;
  onPatch: (mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void) => void;
}) {
  const ab = record.cellularAnalog!.ab;
  return (
    <EnumSelect
      label="A/B Selection"
      value={ab}
      options={AB_OPTIONS}
      onChange={(next) =>
        onPatch((d) => {
          if (d.cellularAnalog) d.cellularAnalog.ab = next;
        })
      }
    />
  );
}
