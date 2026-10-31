import { grpcErrorMessage, grpcErrorStatus, getMscManagementClient } from "@/lib/grpc/client";
import { AdmissionEvent } from "@/lib/proto/events/v1/msc";

export const dynamic = "force-dynamic";

const DEFAULT_LIMIT = 25;
const REQUEST_TIMEOUT_MS = 5_000;

export async function GET(request: Request) {
  const params = new URL(request.url).searchParams;
  const imsis = params.getAll("imsi").filter(Boolean);
  const esnParam = params.get("esn");
  const esn = esnParam ? Number(esnParam) : undefined;
  const meid = params.get("meid") ?? "";
  const limit = Number(params.get("limit") ?? DEFAULT_LIMIT);
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), REQUEST_TIMEOUT_MS);
  try {
    const result = await getMscManagementClient().listAdmissionEvents(
      {
        currentOnly: params.get("current") === "1",
        imsis,
        esn: esn != null && !Number.isNaN(esn) ? esn : undefined,
        meid,
        limit: Number.isNaN(limit) ? DEFAULT_LIMIT : limit,
      },
      { signal: abort.signal },
    );
    return Response.json({
      events: params.get("current") === "1" && !result.currentSnapshot ? [] : result.events.map((event) => AdmissionEvent.toJSON(event)),
      accessMode: result.accessMode,
      registerPolicy: result.registerPolicy,
      authenticationMode: result.authenticationMode,
    });
  } catch (err) {
    return Response.json({ error: grpcErrorMessage(err) }, { status: grpcErrorStatus(err) });
  } finally {
    clearTimeout(timeout);
  }
}
