import {
  PrlAcqRecord,
  PrlExtAcqRecord,
} from "@/lib/proto/hlr/v1/service";
import { ChannelListEditor } from "../shared/ChannelListEditor";

export function CellularCdmaCustomEditor({
  record,
  onPatch,
  error,
}: {
  record: PrlAcqRecord | PrlExtAcqRecord;
  onPatch: (mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void) => void;
  error?: string;
}) {
  return (
    <div>
      <ChannelListEditor
        label="Cellular CDMA channels"
        bits={11}
        maxRows={31}
        values={record.cellularCdmaCustom?.channels ?? []}
        onChange={(next) =>
          onPatch((d) => {
            if (d.cellularCdmaCustom) d.cellularCdmaCustom.channels = next;
          })
        }
      />
      {error && <p className="text-accent-red text-[11px] mt-1">{error}</p>}
    </div>
  );
}
