import { createChannel } from "nice-grpc";
import { describeError } from "../api-error";
import { grpcClients, waitForService } from "./rich-client";
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
  return grpcClients.create(MscManagementServiceDefinition, mscChannel);
}

// Every radio-access, HRPD, PCF, and PDSN management call routes through the
// MSC's NetworkManagementService, which fans out to the addressed base station.
export function getNetworkManagementClient() {
  return grpcClients.create(NetworkManagementServiceDefinition, mscChannel);
}

export async function waitForManagementReady(timeoutMs = MSC_GRPC_READY_TIMEOUT_MS) {
  await waitForService(mscChannel, "/mgmt.v1.NetworkManagementService", timeoutMs);
}

export async function waitForMscReady(timeoutMs = MSC_GRPC_READY_TIMEOUT_MS) {
  await waitForService(mscChannel, "/mgmt.v1.NetworkManagementService", timeoutMs);
}

export function grpcErrorStatus(err: unknown): number {
  return describeError(err).status;
}
