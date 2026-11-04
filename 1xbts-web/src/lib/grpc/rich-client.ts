import { ClientError, Status, createClientFactory, waitForChannelReady, type Channel } from "nice-grpc";
import { errorDetailsClientMiddleware } from "nice-grpc-error-details";

// Every gRPC client goes through this factory so a failed call raises a
// RichClientError holding the server's google.rpc error details.
export const grpcClients = createClientFactory().use(errorDetailsClientMiddleware);

export async function waitForService(channel: Channel, servicePath: string, timeoutMs: number) {
  try {
    await waitForChannelReady(channel, new Date(Date.now() + timeoutMs));
  } catch {
    throw new ClientError(servicePath, Status.UNAVAILABLE, "not reachable");
  }
}
