import { grpcErrorMessage, grpcErrorStatus } from "@/lib/grpc/client";
import { getHlrClient } from "@/lib/grpc/hlr-client";
import {
  MobileSeen,
  MobileSeenFilter,
  subscriberStatusToJSON,
} from "@/lib/proto/hlr/v1/service";
import type { ObservedMobile } from "@/lib/admission";

export const dynamic = "force-dynamic";

const DEFAULT_PAGE_SIZE = 50;
const REQUEST_TIMEOUT_MS = 5_000;
const PSEUDO_ESN_MANUFACTURER = 0x80;
const ESN_MANUFACTURER_SHIFT = 24;

const FILTERS: Record<string, MobileSeenFilter> = {
  all: MobileSeenFilter.MOBILE_SEEN_FILTER_ALL,
  denied: MobileSeenFilter.MOBILE_SEEN_FILTER_CURRENTLY_DENIED,
  restricted: MobileSeenFilter.MOBILE_SEEN_FILTER_CURRENTLY_RESTRICTED,
  ever_denied: MobileSeenFilter.MOBILE_SEEN_FILTER_EVER_DENIED,
};

export async function GET(request: Request) {
  const params = new URL(request.url).searchParams;
  const filter = FILTERS[params.get("filter") ?? "restricted"] ?? FILTERS.restricted;
  const pageSize = Number(params.get("limit") ?? DEFAULT_PAGE_SIZE);
  const offset = Number(params.get("offset") ?? 0);
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), REQUEST_TIMEOUT_MS);
  try {
    const hlr = getHlrClient();
    const seen = await hlr.listMobilesSeen({ filter, pageSize, offset }, { signal: abort.signal });
    const mobiles: ObservedMobile[] = await Promise.all(seen.mobiles.map(async (mobile) => {
      const json = MobileSeen.toJSON(mobile) as ObservedMobile;
      try {
        let resolved = await hlr.resolveSubscriberByIdentity({ identity: { imsi: mobile.imsi, esn: mobile.esn, meid: mobile.meid } }, { signal: abort.signal });
        const genuineEsn = mobile.esn != null && (mobile.esn >>> ESN_MANUFACTURER_SHIFT) !== PSEUDO_ESN_MANUFACTURER ? mobile.esn : undefined;
        if (!resolved.subscriber && (mobile.meid || genuineEsn != null)) {
          resolved = await hlr.resolveSubscriberByHardwareIdentity({ identity: { esn: genuineEsn, meid: mobile.meid } }, { signal: abort.signal });
        }
        if (resolved.subscriber) {
          const target = await hlr.getSubscriber({ subscriberId: resolved.subscriber.subscriberId }, { signal: abort.signal });
          const identity = target.identities.find((identity) => {
            const meidMatch = !!mobile.meid && !!identity.meid && mobile.meid.toUpperCase() === identity.meid.toUpperCase();
            const esnMatch = genuineEsn != null && genuineEsn === identity.esn;
            const meidConflict = !!mobile.meid && !!identity.meid && mobile.meid.toUpperCase() !== identity.meid.toUpperCase();
            const esnConflict = mobile.esn != null && identity.esn != null && mobile.esn !== identity.esn;
            return (meidMatch || esnMatch || (identity.imsi === mobile.imsi && identity.esn === mobile.esn)) && !meidConflict && !esnConflict;
          });
          if (target.subscriber && identity) {
            json.subscriber = {
              subscriberId: target.subscriber.subscriberId,
              phoneNumber: target.subscriber.phoneNumber,
              displayName: target.subscriber.displayName,
              status: subscriberStatusToJSON(target.subscriber.status),
              configuredImsi: identity.imsi,
            };
          }
        }
      } catch {}
      return { ...json, deniedCount: mobile.deniedCount };
    }));
    return Response.json({ mobiles, total: seen.total });
  } catch (err) {
    return Response.json({ error: grpcErrorMessage(err) }, { status: grpcErrorStatus(err) });
  } finally {
    clearTimeout(timeout);
  }
}
