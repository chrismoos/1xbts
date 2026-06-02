import {
  PrlAcqRecord,
  PrlExtAcqRecord,
} from "@/lib/proto/hlr/v1/service";
import { AB_OPTIONS, STD_CHAN_OPTIONS } from "@/lib/prl-options";
import { EnumSelect } from "../shared/EnumSelect";

export function JtacsCdmaStandardEditor({
  record,
  onPatch,
}: {
  record: PrlAcqRecord | PrlExtAcqRecord;
  onPatch: (mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void) => void;
}) {
  const body = record.jtacsCdmaStandard!;
  return (
    <div className="grid grid-cols-2 gap-2">
      <EnumSelect
        label="A/B Selection"
        value={body.ab}
        options={AB_OPTIONS}
        onChange={(next) =>
          onPatch((d) => {
            if (d.jtacsCdmaStandard) d.jtacsCdmaStandard.ab = next;
          })
        }
      />
      <EnumSelect
        label="PRI_SEC"
        value={body.priSec}
        options={STD_CHAN_OPTIONS}
        onChange={(next) =>
          onPatch((d) => {
            if (d.jtacsCdmaStandard) d.jtacsCdmaStandard.priSec = next;
          })
        }
      />
    </div>
  );
}
