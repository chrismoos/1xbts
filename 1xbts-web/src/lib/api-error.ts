import { ClientError, Status } from "nice-grpc-common";
import { RichClientError, type BadRequest, type ErrorInfo } from "nice-grpc-error-details";
import { ErrorReason, errorReasonFromJSON } from "./proto/errors/v1/errors";
import type { ApiErrorBody, ApiErrorField } from "./api-error-body";

export type { ApiErrorBody, ApiErrorField } from "./api-error-body";

const SERVICE_LABELS: Record<string, string> = {
  an: "EV-DO access network",
  bsc: "BSC",
  bts: "BTS",
  events: "event bus",
  femto: "femtocell manager",
  hlr: "HLR",
  msc: "MSC",
  packet: "packet data service",
  pcf: "PCF",
  smsc: "SMSC",
};

const PACKAGE_SERVICES: Record<string, string> = {
  "events.v1": "events",
  "femto_management.v1": "femto",
  "hlr.v1": "hlr",
  "mgmt.v1": "msc",
  "msc_management.v1": "msc",
  "smsc.v1": "smsc",
};

const CODE_NAMES: Partial<Record<Status, string>> = {
  [Status.CANCELLED]: "cancelled",
  [Status.UNKNOWN]: "unknown",
  [Status.INVALID_ARGUMENT]: "invalid_argument",
  [Status.DEADLINE_EXCEEDED]: "timeout",
  [Status.NOT_FOUND]: "not_found",
  [Status.ALREADY_EXISTS]: "already_exists",
  [Status.PERMISSION_DENIED]: "permission_denied",
  [Status.RESOURCE_EXHAUSTED]: "resource_exhausted",
  [Status.FAILED_PRECONDITION]: "failed_precondition",
  [Status.ABORTED]: "aborted",
  [Status.OUT_OF_RANGE]: "invalid_argument",
  [Status.UNIMPLEMENTED]: "not_supported",
  [Status.INTERNAL]: "internal",
  [Status.UNAVAILABLE]: "unavailable",
  [Status.DATA_LOSS]: "internal",
  [Status.UNAUTHENTICATED]: "unauthenticated",
};

const HTTP_STATUS: Partial<Record<Status, number>> = {
  [Status.CANCELLED]: 499,
  [Status.INVALID_ARGUMENT]: 400,
  [Status.OUT_OF_RANGE]: 400,
  [Status.DEADLINE_EXCEEDED]: 504,
  [Status.NOT_FOUND]: 404,
  [Status.ALREADY_EXISTS]: 409,
  [Status.FAILED_PRECONDITION]: 409,
  [Status.ABORTED]: 409,
  [Status.PERMISSION_DENIED]: 403,
  [Status.UNAUTHENTICATED]: 401,
  [Status.RESOURCE_EXHAUSTED]: 429,
  [Status.UNIMPLEMENTED]: 501,
  [Status.UNAVAILABLE]: 503,
};

function serviceLabel(service: string | undefined): string {
  return (service && SERVICE_LABELS[service]) || "core network";
}

function sentence(text: string): string {
  const trimmed = text.trim();
  if (!trimmed) return trimmed;
  const capitalized = trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
  return /[.!?]$/.test(capitalized) ? capitalized : `${capitalized}.`;
}

function serviceFromPath(path: string): string | undefined {
  const pkg = path.replace(/^\//, "").split("/")[0] ?? "";
  const name = pkg.slice(0, pkg.lastIndexOf("."));
  return PACKAGE_SERVICES[name];
}

interface ReasonContext {
  message: string;
  service: string;
  metadata: Record<string, string>;
}

// Text for reasons where the service's own message needs guidance or names
// that only the web UI can add. Every other reason shows the service's message.
const REASON_TEXT: Partial<Record<ErrorReason, (context: ReasonContext) => string>> = {
  [ErrorReason.ERROR_REASON_INTERNAL]: ({ service }) =>
    `Something went wrong in the ${serviceLabel(service)}. Its log has the details.`,
  [ErrorReason.ERROR_REASON_DATABASE_UNAVAILABLE]: ({ service }) =>
    `The ${serviceLabel(service)} could not read or write its database. Check that the database is running.`,
  [ErrorReason.ERROR_REASON_SERVICE_STOPPING]: ({ service }) =>
    `The ${serviceLabel(service)} is restarting or shutting down. Try again in a moment.`,
  [ErrorReason.ERROR_REASON_NOT_SUPPORTED]: ({ message }) => sentence(message),
  [ErrorReason.ERROR_REASON_BASE_STATION_UNREACHABLE]: ({ metadata }) =>
    `${metadata.base_station ? `Base station ${metadata.base_station}` : "The base station"} could not be reached. Check that it is running and connected to the MSC.`,
  [ErrorReason.ERROR_REASON_BASE_STATION_NOT_ATTACHED]: ({ message }) =>
    `${sentence(message)} Check that the base station is running and connected to the MSC.`,
  [ErrorReason.ERROR_REASON_SERVICE_UNREACHABLE]: ({ metadata }) =>
    `The ${serviceLabel(metadata.service)} could not be reached. Check that it is running.`,
  [ErrorReason.ERROR_REASON_CELL_UNREACHABLE]: ({ message }) =>
    `${sentence(message)} Check that the BTS is running and attached to the BSC.`,
  [ErrorReason.ERROR_REASON_BSC_UNAVAILABLE]: () =>
    "The MSC has lost its link to the BSC. Check that the base station is running.",
  [ErrorReason.ERROR_REASON_HLR_UNAVAILABLE]: () =>
    "The MSC could not reach the HLR. Check that the HLR and its database are running.",
  [ErrorReason.ERROR_REASON_SUBSCRIBER_NOT_FOUND]: () =>
    "This subscriber does not exist or was deleted.",
  [ErrorReason.ERROR_REASON_IDENTITY_IN_USE]: ({ metadata, message }) =>
    metadata.identity && metadata.value
      ? `${metadata.identity} ${metadata.value} already belongs to another subscriber.`
      : sentence(message),
  [ErrorReason.ERROR_REASON_SUBSCRIBER_NOT_REGISTERED]: () =>
    "The handset has not registered yet. Turn it on in coverage and try again.",
  [ErrorReason.ERROR_REASON_SUBSCRIBER_BUSY]: () => "The subscriber is already on a call.",
  [ErrorReason.ERROR_REASON_SUBSCRIBER_INACTIVE]: () =>
    "The subscriber is suspended or disabled. Set it to active first.",
  [ErrorReason.ERROR_REASON_PRL_NOT_FOUND]: () => "This PRL does not exist or was deleted.",
  [ErrorReason.ERROR_REASON_PRL_IN_USE]: ({ metadata, message }) => {
    const count = Number(metadata.subscriber_count);
    return Number.isFinite(count) && count > 0
      ? `It is assigned to ${count} subscriber${count === 1 ? "" : "s"}. Assign ${count === 1 ? "it" : "them"} a different PRL first.`
      : sentence(message);
  },
  [ErrorReason.ERROR_REASON_PACKET_SESSION_NOT_FOUND]: () =>
    "This packet data session has ended or does not exist.",
  [ErrorReason.ERROR_REASON_EVDO_SESSION_NOT_FOUND]: () =>
    "This EV-DO session has closed or does not exist.",
  [ErrorReason.ERROR_REASON_EVDO_NOT_ENABLED]: () =>
    "EV-DO is not enabled on this cell. Turn it on in the BTS config.",
  [ErrorReason.ERROR_REASON_OTASP_DISABLED]: () =>
    "OTASP is turned off on the MSC. Turn it on in the MSC config.",
  [ErrorReason.ERROR_REASON_SMS_NOT_FOUND]: () => "This message does not exist or was deleted.",
  [ErrorReason.ERROR_REASON_OTASP_SESSION_NOT_FOUND]: () => "This OTASP session does not exist or was deleted.",
  [ErrorReason.ERROR_REASON_ACCESS_CLOSED]: () =>
    "Access is closed, and the destination is not a provisioned subscriber. Provision it or open access first.",
  [ErrorReason.ERROR_REASON_CALL_NOT_FOUND]: () => "The call has already ended.",
  [ErrorReason.ERROR_REASON_FEMTOCELL_NO_MANAGEMENT_PATH]: () =>
    "The femtocell has not brought up its management tunnel yet. Wait for it to connect to the security gateway.",
  [ErrorReason.ERROR_REASON_FEMTOCELL_AUTH_FAILED]: ({ message }) =>
    `${sentence(message)} Check the connection request credentials in its profile.`,
};

function infoOf(err: RichClientError): ErrorInfo | undefined {
  return err.extra.find((detail): detail is ErrorInfo => detail.$type === "google.rpc.ErrorInfo");
}

function fieldsOf(err: RichClientError): ApiErrorField[] {
  const bad = err.extra.find((detail): detail is BadRequest => detail.$type === "google.rpc.BadRequest");
  return bad?.fieldViolations.map((violation) => ({ field: violation.field, description: violation.description })) ?? [];
}

function codeMessage(code: Status, details: string, service: string | undefined): string {
  const label = serviceLabel(service);
  switch (code) {
    case Status.UNAVAILABLE:
      return `The ${label} could not be reached. Check that it is running.`;
    case Status.DEADLINE_EXCEEDED:
      return `The ${label} took too long to respond.`;
    case Status.UNIMPLEMENTED:
      return `The ${label} does not support this.`;
    case Status.INTERNAL:
    case Status.UNKNOWN:
    case Status.DATA_LOSS:
      return `Something went wrong in the ${label}. Its log has the details.`;
    case Status.CANCELLED:
      return "The request was cancelled.";
    default:
      return details ? sentence(details) : `The ${label} refused the request.`;
  }
}

function isAbort(err: unknown): boolean {
  return (
    err instanceof Error &&
    (err.name === "AbortError" || /aborted/i.test(err.message) || (err as { code?: string }).code === "ABORT_ERR")
  );
}

function withTitle(body: Omit<ApiErrorBody, "error">, action?: string): ApiErrorBody {
  const title = action ? `Couldn't ${action}` : undefined;
  return { ...body, title, error: title ? `${title}. ${body.message}` : body.message };
}

export function describeError(err: unknown, action?: string): { body: ApiErrorBody; status: number } {
  if (err instanceof ClientError) {
    const service = serviceFromPath(err.path);
    const detail = `${err.path} ${Status[err.code]}: ${err.details}`;
    const status = HTTP_STATUS[err.code] ?? 502;
    const code = CODE_NAMES[err.code] ?? "error";
    if (err instanceof RichClientError) {
      const info = infoOf(err);
      if (info) {
        const reason = errorReasonFromJSON(info.reason);
        const origin = info.domain || service || "";
        const metadata = { ...info.metadata };
        const text = REASON_TEXT[reason];
        const message = text
          ? text({ message: err.details, service: origin, metadata })
          : sentence(err.details) || codeMessage(err.code, "", origin);
        const fields = fieldsOf(err);
        return {
          body: withTitle(
            {
              message,
              code,
              reason: info.reason.replace(/^ERROR_REASON_/, ""),
              service: origin,
              metadata,
              fields: fields.length ? fields : undefined,
              detail,
            },
            action,
          ),
          status,
        };
      }
    }
    return {
      body: withTitle({ message: codeMessage(err.code, err.details, service), code, service, detail }, action),
      status,
    };
  }
  if (isAbort(err)) {
    return {
      body: withTitle({ message: "The core network took too long to respond.", code: "timeout" }, action),
      status: 504,
    };
  }
  const detail = err instanceof Error ? err.message : String(err);
  return {
    body: withTitle({ message: "Something went wrong in the web server. Its log has the details.", code: "internal", detail }, action),
    status: 500,
  };
}

export function apiError(err: unknown, action?: string, extra?: Record<string, unknown>): Response {
  const { body, status } = describeError(err, action);
  if (status >= 500) console.error(`API error${action ? ` (${action})` : ""}: ${body.detail ?? body.message}`);
  return Response.json(extra ? { ...extra, ...body } : body, { status });
}

export function badRequest(message: string, field?: string, status = 400): Response {
  const text = sentence(message);
  const body: ApiErrorBody = {
    error: text,
    message: text,
    code: "invalid_argument",
    fields: field ? [{ field, description: text }] : undefined,
  };
  return Response.json(body, { status });
}

export function notFound(message: string): Response {
  const text = sentence(message);
  return Response.json({ error: text, message: text, code: "not_found" } satisfies ApiErrorBody, { status: 404 });
}
