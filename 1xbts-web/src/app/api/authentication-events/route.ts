import { grpcErrorMessage, grpcErrorStatus } from "@/lib/grpc/client";
import { getMscManagementClient } from "@/lib/grpc/client";
import { AuthenticationEvent } from "@/lib/proto/events/v1/msc";

export const dynamic = "force-dynamic";

const DEFAULT_LIMIT = 25;
const REQUEST_TIMEOUT_MS = 5_000;

export async function GET(request: Request) {
  const params = new URL(request.url).searchParams;
  const imsis = params.getAll("imsi").filter(Boolean);
  const esnParam = params.get("esn");
  const esn = esnParam ? Number(esnParam) : undefined;
  const limit = Number(params.get("limit") ?? DEFAULT_LIMIT);
  if (imsis.length === 0 && (esn == null || Number.isNaN(esn))) {
    return Response.json({ events: [] });
  }
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), REQUEST_TIMEOUT_MS);
  try {
    const result = await getMscManagementClient().listAuthenticationEvents(
      { imsis, esn: esn != null && !Number.isNaN(esn) ? esn : undefined, limit },
      { signal: abort.signal }
    );
    return Response.json({
      events: result.events.map((event) => AuthenticationEvent.toJSON(event)),
    });
  } catch (err) {
    return Response.json(
      { error: grpcErrorMessage(err) },
      { status: grpcErrorStatus(err) }
    );
  } finally {
    clearTimeout(timeout);
  }
}
