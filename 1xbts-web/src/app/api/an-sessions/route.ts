import { peerRequestFromUrl, selectorFromUrl } from "@/lib/cell";
import { getNetworkManagementClient } from "@/lib/grpc/client";
import { evdoCells, isUnscoped } from "@/lib/grpc/an-aggregate";
import {
  GetSessionsResponse,
  SessionState,
  type Session,
} from "@/lib/proto/an/v1/service";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  const request_ = { stateFilter: SessionState.SESSION_STATE_UNSPECIFIED };
  try {
    const client = getNetworkManagementClient();
    // A request with no cell aggregates HRPD sessions across every EV-DO cell.
    if (isUnscoped(request.url)) {
      const sessions: Session[] = [];
      for (const ref of await evdoCells(abort.signal)) {
        const r = await client.getAnSessions(
          { ...ref, request: request_ },
          { signal: abort.signal },
        );
        sessions.push(...(r.sessions ?? []));
      }
      return Response.json(GetSessionsResponse.toJSON({ sessions }));
    }
    const result = await client.getAnSessions(
      {
        selector: selectorFromUrl(request.url),
        cell: peerRequestFromUrl(request.url),
        request: request_,
      },
      { signal: abort.signal },
    );
    return Response.json(GetSessionsResponse.toJSON(result));
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
