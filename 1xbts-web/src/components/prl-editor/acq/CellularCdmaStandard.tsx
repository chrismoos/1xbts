import {
  PrlAcqRecord,
  PrlExtAcqRecord,
} from "@/lib/proto/hlr/v1/service";
import { AB_OPTIONS, STD_CHAN_OPTIONS } from "@/lib/prl-options";
import { EnumSelect } from "../shared/EnumSelect";

export function CellularCdmaStandardEditor({
  record,
  onPatch,
}: {
  record: PrlAcqRecord | PrlExtAcqRecord;
  onPatch: (mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void) => void;
}) {
  const body = record.cellularCdmaStandard!;
  return (
    <div className="grid grid-cols-2 gap-2">
      <EnumSelect
        label="A/B Selection"
        value={body.ab}
        options={AB_OPTIONS}
        onChange={(next) =>
          onPatch((d) => {
            if (d.cellularCdmaStandard) d.cellularCdmaStandard.ab = next;
          })
        }
      />
      <EnumSelect
        label="PRI_SEC"
        value={body.priSec}
        options={STD_CHAN_OPTIONS}
        onChange={(next) =>
          onPatch((d) => {
            if (d.cellularCdmaStandard) d.cellularCdmaStandard.priSec = next;
          })
        }
      />
    </div>
  );
}
