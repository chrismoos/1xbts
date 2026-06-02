import {
  PrlAcqRecord,
  PrlExtAcqRecord,
} from "@/lib/proto/hlr/v1/service";
import { ChannelListEditor } from "../shared/ChannelListEditor";

export function JtacsCdmaCustomEditor({
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
        label="JTACS CDMA channels"
        bits={11}
        maxRows={31}
        values={record.jtacsCdmaCustom?.channels ?? []}
        onChange={(next) =>
          onPatch((d) => {
            if (d.jtacsCdmaCustom) d.jtacsCdmaCustom.channels = next;
          })
        }
      />
      {error && <p className="text-accent-red text-[11px] mt-1">{error}</p>}
    </div>
  );
}
