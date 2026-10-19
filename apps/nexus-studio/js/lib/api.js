// Thin client for the Studio server. Writes carry the session token the
// server puts into index.html, so other local pages cannot change settings.

let token = document.querySelector('meta[name="nexus-token"]')?.content ?? "";

/** A restarted server has a new token: read it from a fresh index.html. */
async function renewToken() {
  try {
    const page = await (await fetch("/", { cache: "no-store" })).text();
    const fresh = new DOMParser().parseFromString(page, "text/html").querySelector('meta[name="nexus-token"]')?.content;
    if (fresh && fresh !== token) {
      token = fresh;
      return true;
    }
  } catch {
    /* server gone; the caller reports the original error */
  }
  return false;
}

export class ApiError extends Error {
  constructor(message, status, body) {
    super(message);
    this.status = status;
    this.body = body;
  }
}

async function request(method, path, body, { raw = false, retried = false, headers: extra = {} } = {}) {
  const headers = { ...extra };
  if (method !== "GET") {
    headers["X-Nexus-Studio"] = "1";
    headers["X-Nexus-Studio-Token"] = token;
  }
  let payload;
  if (body instanceof Blob || body instanceof ArrayBuffer) {
    payload = body;
    headers["Content-Type"] = "application/octet-stream";
  } else if (body !== undefined) {
    payload = JSON.stringify(body);
    headers["Content-Type"] = "application/json";
  }
  const response = await fetch(path, { method, headers, body: payload, cache: "no-store" });
  // The retry repeats the request unchanged. A config save carries the
  // revision it started from (If-Match), so a stale tab replaying it after
  // a server restart gets 409 instead of overwriting newer changes.
  if (response.status === 403 && method !== "GET" && !retried && (await renewToken())) {
    return request(method, path, body, { raw, retried: true, headers: extra });
  }
  if (raw) {
    if (!response.ok) throw new ApiError(`${response.status} ${response.statusText}`, response.status);
    return response;
  }
  const text = await response.text();
  let data = {};
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    throw new ApiError(text || response.statusText, response.status);
  }
  if (!response.ok || data.ok === false) {
    throw new ApiError(data.message || response.statusText, response.status, data);
  }
  return data;
}

export const api = {
  get: (path) => request("GET", path),
  post: (path, body, options) => request("POST", path, body ?? {}, options),
  del: (path) => request("DELETE", path),
  /** PNG of `page` rendered by the same engine as the strip. */
  async preview(page, theme, settings, extra = {}) {
    const response = await request("POST", "/api/preview", { page, theme, settings, ...extra }, { raw: true });
    return response.blob();
  },
  async frame() {
    const response = await request("GET", "/api/frame.png", undefined, { raw: true });
    return response.blob();
  },
  download(path) {
    const a = document.createElement("a");
    a.href = path;
    a.download = "";
    document.body.append(a);
    a.click();
    a.remove();
  },
};
