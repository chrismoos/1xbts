import { createChannel, createClient, waitForChannelReady } from "nice-grpc";
import { BscServiceDefinition } from "../proto/bsc/v1/service";
import { BscManagementServiceDefinition } from "../proto/bsc_management/v1/service";
import { BtsManagementServiceDefinition } from "../proto/bts_management/v1/service";
import { ManagementFacadeServiceDefinition } from "../proto/management/v1/service";
import { MscManagementServiceDefinition } from "../proto/msc_management/v1/service";
import { PcfManagementServiceDefinition } from "../proto/pcf_management/v1/service";
import { PdsnManagementServiceDefinition } from "../proto/pdsn_management/v1/service";

// BSC gRPC address — serves radio-access management, streaming events, HLR, SMSC, PCF/PDSN.
const BSC_GRPC_ADDRESS =
  process.env.MANAGEMENT_GRPC_ADDRESS ||
  process.env.BSC_GRPC_ADDRESS ||
  "127.0.0.1:17016";

// MSC gRPC address — serves initiate_call, list_calls, and send_sms.
const MSC_GRPC_ADDRESS =
  process.env.MSC_GRPC_ADDRESS ||
  "127.0.0.1:17017";

const parsedTimeoutMs = Number(process.env.BSC_GRPC_READY_TIMEOUT_MS || "1500");
const BSC_GRPC_READY_TIMEOUT_MS = Number.isFinite(parsedTimeoutMs)
  ? parsedTimeoutMs
  : 1500;

// Shared BSC channel — reused across all requests to avoid accumulating connections.
export const channel = createChannel(BSC_GRPC_ADDRESS, undefined, {
  "grpc.initial_reconnect_backoff_ms": 100,
  "grpc.max_reconnect_backoff_ms": 1000,
});

// Separate MSC channel — initiate_call and list_calls go directly to the MSC.
const mscChannel = createChannel(MSC_GRPC_ADDRESS, undefined, {
  "grpc.initial_reconnect_backoff_ms": 100,
  "grpc.max_reconnect_backoff_ms": 1000,
});

export function getBscClient() {
  return createClient(BscServiceDefinition, channel);
}

export function getManagementFacadeClient() {
  return createClient(ManagementFacadeServiceDefinition, channel);
}

export function getBtsManagementClient() {
  return createClient(BtsManagementServiceDefinition, channel);
}

export function getBscManagementClient() {
  return createClient(BscManagementServiceDefinition, channel);
}

export function getMscManagementClient() {
  return createClient(MscManagementServiceDefinition, mscChannel);
}

export function getPcfManagementClient() {
  return createClient(PcfManagementServiceDefinition, channel);
}

export function getPdsnManagementClient() {
  return createClient(PdsnManagementServiceDefinition, channel);
}

export async function waitForBscReady(timeoutMs = BSC_GRPC_READY_TIMEOUT_MS) {
  await waitForChannelReady(channel, new Date(Date.now() + timeoutMs));
}
