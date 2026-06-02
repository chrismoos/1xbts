import {
  PrlAcqRecord,
  PrlExtAcqRecord,
} from "@/lib/proto/hlr/v1/service";
import { ChannelListEditor } from "../shared/ChannelListEditor";

export function BandClass6UsingChannelsEditor({
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
        label="Band Class 6 (2 GHz) CDMA channels"
        bits={11}
        maxRows={31}
        values={record.bandClass6UsingChannels?.channels ?? []}
        onChange={(next) =>
          onPatch((d) => {
            if (d.bandClass6UsingChannels)
              d.bandClass6UsingChannels.channels = next;
          })
        }
      />
      {error && <p className="text-accent-red text-[11px] mt-1">{error}</p>}
    </div>
  );
}
