// Host functions for SystemHttp (http.emel) on JS. Sends requests with `fetch`.
//
// `send` never rejects: a failure becomes an outcome object that the handler turns into an error.
// Lists cannot cross the boundary, so the request is built by `newRequest` and `withHeader`, and
// the response headers are read one by one by index.
//
// The content-type is left to fetch, which sends `text/plain;charset=UTF-8` for a string body. The
// body is decoded with the default `ignoreBOM: false`, so a leading BOM is dropped.

const RESPONSE = 0;
const INVALID_URL = 1;
const CONNECTION_FAILED = 2;
const RESPONSE_NOT_TEXT = 3;

/** A request with no headers. */
export const newRequest = (method, url, body) => ({ method, url, body, headers: [] });

/** A copy of `req` with the header added after the others. */
export const withHeader = (req, name, value) => ({ ...req, headers: [...req.headers, [name, value]] });

/**
 * Sends `req` and reads the whole body as UTF-8. `signal` is the AbortSignal the runtime appends
 * to the arguments of a suspending external function (spec 12.2); it may be undefined. Aborting
 * gives a ConnectionFailed outcome ("This operation was aborted"), which the interrupted fiber
 * drops.
 */
export async function send(req, signal) {
  let url;
  try {
    url = new URL(req.url);
  } catch {
    return failure(INVALID_URL, "");
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    return failure(INVALID_URL, "");
  }
  // GET and HEAD cannot have a body in fetch; an empty body is sent as no body.
  const body = req.body === "" && (req.method === "GET" || req.method === "HEAD") ? undefined : req.body;
  let res;
  let bytes;
  try {
    res = await fetch(url, { method: req.method, headers: req.headers, body, redirect: "follow", signal });
    bytes = await res.arrayBuffer();
  } catch (e) {
    return failure(CONNECTION_FAILED, describe(e));
  }
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return failure(RESPONSE_NOT_TEXT, "");
  }
  return { outcome: RESPONSE, message: "", status: res.status, headers: responseHeaders(res.headers), body: text };
}

const failure = (outcome, message) => ({ outcome, message, status: 0, headers: [], body: "" });

/** The error message, followed by the cause's code and message on Node (`fetch failed: ECONNREFUSED ...`). */
function describe(e) {
  const message = e instanceof Error ? e.message : String(e);
  const cause = e instanceof Error ? e.cause : undefined;
  if (cause == null) return message;
  const detail = [cause.code, cause.message].filter((s) => typeof s === "string" && s !== "").join(" ");
  return detail === "" ? message : `${message}: ${detail}`;
}

/**
 * The headers as `[name, value]` pairs: lower-case names in name order, values of the same name
 * joined with ", ", and each set-cookie as its own pair.
 */
function responseHeaders(headers) {
  const pairs = [];
  for (const [name, value] of headers) {
    if (name !== "set-cookie") pairs.push([name, value]);
  }
  const cookies = typeof headers.getSetCookie === "function"
    ? headers.getSetCookie()
    : headers.get("set-cookie") === null ? [] : [headers.get("set-cookie")];
  for (const cookie of cookies) pairs.push(["set-cookie", cookie]);
  // A stable sort keeps the set-cookie values in the received order.
  return pairs.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
}

export const outcome = (res) => res.outcome;
export const message = (res) => res.message;
export const status = (res) => res.status;
export const body = (res) => res.body;
export const headerCount = (res) => res.headers.length;
export const headerName = (res, index) => res.headers[index][0];
export const headerValue = (res, index) => res.headers[index][1];
