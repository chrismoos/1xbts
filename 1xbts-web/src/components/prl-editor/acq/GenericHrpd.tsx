import {
  PrlAcqRecord,
  PrlExtAcqRecord,
  PrlBandClassChannel,
} from "@/lib/proto/hlr/v1/service";
import { BandClassChannelList } from "./Generic1xIs95";

export function GenericHrpdEditor({
  record,
  onPatch,
}: {
  record: PrlAcqRecord | PrlExtAcqRecord;
  onPatch: (mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void) => void;
}) {
  if (!("genericHrpd" in record) || !record.genericHrpd) return null;
  const entries = record.genericHrpd.entries;

  const add = () =>
    onPatch((d) => {
      if ("genericHrpd" in d && d.genericHrpd)
        d.genericHrpd.entries.push({ bandClass: 0, channelNumber: 0 });
    });

  const removeAt = (i: number) =>
    onPatch((d) => {
      if ("genericHrpd" in d && d.genericHrpd)
        d.genericHrpd.entries.splice(i, 1);
    });

  const update = (i: number, patch: Partial<PrlBandClassChannel>) =>
    onPatch((d) => {
      if ("genericHrpd" in d && d.genericHrpd) {
        Object.assign(d.genericHrpd.entries[i], patch);
      }
    });

  return (
    <BandClassChannelList
      entries={entries}
      add={add}
      remove={removeAt}
      update={update}
      label="HRPD entries"
    />
  );
}
