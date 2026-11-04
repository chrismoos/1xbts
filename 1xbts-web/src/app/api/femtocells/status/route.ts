import { getFemtoManagementClient } from "@/lib/grpc/femto-client";
import { apiError, badRequest } from "@/lib/api-error";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const url = new URL(request.url);
  const baseStation = url.searchParams.get("baseStation");
  const peerId = url.searchParams.get("peerId");
  if (!baseStation || !peerId) {
    return badRequest("Choose a base station and a femtocell");
  }
  try {
    const status = await getFemtoManagementClient(baseStation).getFemtoCellStatus({ peerId });
    return Response.json(status);
  } catch (error) {
    return apiError(error, "load the femtocell status");
  }
}
