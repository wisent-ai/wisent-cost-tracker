import { request as httpRequest, type ClientRequest, type IncomingHttpHeaders } from 'node:http';
import { request as httpsRequest } from 'node:https';

export interface HttpReply {
  status: number;
  headers: IncomingHttpHeaders;
  body: string;
}

export class SupabaseRequestError extends Error {
  constructor(
    readonly operation: string,
    readonly method: string,
    readonly url: string,
    readonly statusCode: number | undefined,
    readonly responseBody: string,
    cause: unknown,
  ) {
    const detail = cause instanceof Error ? cause.message : String(cause);
    super(`SupabaseSink ${operation} ${method} ${url} failed: ${detail}; status: ${statusCode ?? 'not received'}; body: ${responseBody}`, { cause });
    this.name = 'SupabaseRequestError';
  }
}

/** Read the entire response through Node's public HTTP interface. */
export function exchange(url: URL, operation: string, method: string,
  headers: Record<string, string>, body?: string): Promise<HttpReply> {
  const { promise, resolve, reject } = Promise.withResolvers<HttpReply>();
  let received = '';
  let status: number | undefined;
  let request: ClientRequest | undefined;
  let settled = false;
  const failed = (cause: unknown) => {
    if (settled) return;
    settled = true;
    reject(new SupabaseRequestError(operation, method, url.href, status, received, cause));
  };
  try {
    const send = url.protocol === 'https:' ? httpsRequest : url.protocol === 'http:' ? httpRequest : undefined;
    if (!send) throw new Error(`unsupported protocol ${url.protocol}`);
    headers['Accept-Encoding'] = 'identity';
    if (body !== undefined) headers['Content-Length'] = String(Buffer.byteLength(body));
    request = send(url, { method, headers }, (response) => {
      status = response.statusCode;
      response.setEncoding('utf8');
      response.on('data', (chunk: string) => { received += chunk; });
      response.once('error', failed);
      response.once('close', () => {
        if (!response.complete) failed(new Error('response closed before the complete body arrived'));
      });
      response.once('end', () => {
        if (!response.complete || status === undefined) {
          failed(new Error('incomplete HTTP response'));
        } else if (status < 200 || status >= 300) {
          failed(new Error(`HTTP ${status}`));
        } else {
          settled = true;
          resolve({ status, headers: response.headers, body: received });
        }
      });
    });
    request.once('error', failed);
    request.end(body);
  } catch (cause) {
    failed(cause);
    request?.destroy();
  }
  return promise;
}

export function jsonArray<T>(reply: HttpReply, operation: string, url: URL): T[] {
  try {
    const value: unknown = JSON.parse(reply.body);
    if (!Array.isArray(value)) throw new Error('expected a JSON array');
    return value as T[];
  } catch (cause) {
    throw new SupabaseRequestError(operation, 'GET', url.href, reply.status, reply.body, cause);
  }
}
