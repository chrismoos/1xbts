import { isApiErrorBody, type ApiErrorBody } from "./api-error-body";

export class ApiError extends Error {
  readonly body: ApiErrorBody;
  readonly status: number;

  constructor(body: ApiErrorBody, status: number) {
    super(body.error);
    this.name = "ApiError";
    this.body = body;
    this.status = status;
  }
}

function plainBody(message: string, code: string, title?: string): ApiErrorBody {
  return { error: title ? `${title}. ${message}` : message, title, message, code };
}

export async function readApiJson<T>(response: Response, action?: string): Promise<T> {
  const title = action ? `Couldn't ${action}` : undefined;
  const text = await response.text();
  let data: unknown;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    throw new ApiError(
      plainBody(
        response.ok
          ? "The web server sent a response that could not be read."
          : `The web server returned HTTP ${response.status}.`,
        "invalid_response",
        title,
      ),
      response.status,
    );
  }
  if (isApiErrorBody(data)) {
    const body = data.title || !title ? data : { ...data, title, error: `${title}. ${data.error}` };
    throw new ApiError(body, response.status);
  }
  if (!response.ok) {
    throw new ApiError(plainBody(`The web server returned HTTP ${response.status}.`, "http_error", title), response.status);
  }
  return data as T;
}

export async function apiFetch<T>(input: RequestInfo | URL, init?: RequestInit, action?: string): Promise<T> {
  let response: Response;
  try {
    response = await fetch(input, init);
  } catch (err) {
    if (err instanceof DOMException && err.name === "AbortError") throw err;
    throw new ApiError(
      plainBody(
        "The web server could not be reached. Check your connection and that the web server is running.",
        "network",
        action ? `Couldn't ${action}` : undefined,
      ),
      0,
    );
  }
  return readApiJson<T>(response, action);
}

export function errorBodyOf(err: unknown, action?: string): ApiErrorBody {
  if (err instanceof ApiError) return err.body;
  if (isApiErrorBody(err)) return err;
  const title = action ? `Couldn't ${action}` : undefined;
  // A page's own Error holds text written for the user. A runtime error
  // from the browser does not, so it goes under the details toggle.
  if (typeof err === "string") return plainBody(err, "client_error", title);
  if (err instanceof Error && !(err instanceof TypeError || err instanceof SyntaxError || err instanceof RangeError)) {
    return plainBody(err.message, "client_error", title);
  }
  const detail = err instanceof Error ? err.message : undefined;
  return { ...plainBody("Something went wrong in this page.", "client_error", title), detail };
}

export function errorText(err: unknown, action?: string): string {
  return errorBodyOf(err, action).error;
}

export function fieldErrorsOf<K extends string>(err: unknown, names: Record<string, K>): Partial<Record<K, string>> {
  const errors: Partial<Record<K, string>> = {};
  for (const field of errorBodyOf(err).fields ?? []) {
    const name = names[field.field];
    if (name && !errors[name]) errors[name] = field.description;
  }
  return errors;
}
