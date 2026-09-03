import {
  ClientError,
  Status,
  createChannel,
  createClient,
  waitForChannelReady,
} from "nice-grpc";
import { NetworkManagementServiceDefinition } from "../proto/mgmt/v1/service";
import { MscManagementServiceDefinition } from "../proto/msc_management/v1/service";

// MSC gRPC address — the single management front door. It serves initiate_call,
// list_calls, send_sms, and all radio, HRPD, and packet management through
// NetworkManagementService.
const MSC_GRPC_ADDRESS = process.env.MSC_GRPC_ADDRESS || "127.0.0.1:17017";

// Host the MSC is reached at. Services co-located with it (HLR, SMSC) default
// to the same host with their own port, so a deployment that sets only the MSC
// address still reaches them.
const MSC_HOST = MSC_GRPC_ADDRESS.split(":")[0] || "127.0.0.1";

/// Address of a service co-located with the MSC, on the MSC host and `port`.
export function coLocatedServiceAddress(port: number): string {
  return `${MSC_HOST}:${port}`;
}

const parsedTimeoutMs = Number(process.env.MSC_GRPC_READY_TIMEOUT_MS || "1500");
const MSC_GRPC_READY_TIMEOUT_MS = Number.isFinite(parsedTimeoutMs)
  ? parsedTimeoutMs
  : 1500;

// Shared MSC channel, reused across requests to avoid accumulating connections.
const mscChannel = createChannel(MSC_GRPC_ADDRESS, undefined, {
  "grpc.initial_reconnect_backoff_ms": 100,
  "grpc.max_reconnect_backoff_ms": 1000,
});

export function getMscManagementClient() {
  return createClient(MscManagementServiceDefinition, mscChannel);
}

// Every radio-access, HRPD, PCF, and PDSN management call routes through the
// MSC's NetworkManagementService, which fans out to the addressed base station.
export function getNetworkManagementClient() {
  return createClient(NetworkManagementServiceDefinition, mscChannel);
}

export async function waitForManagementReady(timeoutMs = MSC_GRPC_READY_TIMEOUT_MS) {
  await waitForChannelReady(mscChannel, new Date(Date.now() + timeoutMs));
}

// Generic gRPC -> HTTP status mapping for API route error responses.
// Reads `ClientError.code` (the structured status from the server);
// anything that isn't a ClientError is treated as an upstream failure.
export function grpcErrorStatus(err: unknown): number {
  if (err instanceof ClientError) {
    switch (err.code) {
      case Status.INVALID_ARGUMENT:
        return 400;
      case Status.NOT_FOUND:
        return 404;
      default:
        return 502;
    }
  }
  return 502;
}

export function grpcErrorMessage(err: unknown): string {
  if (err instanceof ClientError) return err.details;
  if (err instanceof Error) return err.message;
  return "unknown error";
}
