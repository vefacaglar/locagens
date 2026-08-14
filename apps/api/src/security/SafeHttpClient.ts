import dns from "node:dns/promises";
import http from "node:http";
import https from "node:https";
import net from "node:net";

const MAX_REDIRECTS = 5;
const MAX_BODY_BYTES = 2 * 1024 * 1024;

export interface SafeHttpResponse {
  status: number;
  ok: boolean;
  contentType: string;
  finalUrl: string;
  body: string;
}

function isPrivateIpv4(address: string): boolean {
  const parts = address.split(".").map(Number);
  if (parts.length !== 4 || parts.some(n => !Number.isInteger(n) || n < 0 || n > 255)) return true;
  const [a, b] = parts;
  return a === 0 || a === 10 || a === 127 || a >= 224 ||
    (a === 100 && b >= 64 && b <= 127) ||
    (a === 169 && b === 254) ||
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && (b === 0 || b === 168)) ||
    (a === 198 && (b === 18 || b === 19 || b === 51)) ||
    (a === 203 && b === 0);
}

function isPrivateIpv6(address: string): boolean {
  const normalized = address.toLowerCase().split("%", 1)[0];
  const dottedTail = normalized.match(/(\d+\.\d+\.\d+\.\d+)$/)?.[1];
  let expandedInput = normalized;
  if (dottedTail) {
    const octets = dottedTail.split(".").map(Number);
    if (octets.length !== 4 || octets.some(value => !Number.isInteger(value) || value < 0 || value > 255)) return true;
    const hexTail = `${((octets[0] << 8) | octets[1]).toString(16)}:${((octets[2] << 8) | octets[3]).toString(16)}`;
    expandedInput = normalized.slice(0, -dottedTail.length) + hexTail;
  }

  const halves = expandedInput.split("::");
  if (halves.length > 2) return true;
  const left = halves[0] ? halves[0].split(":") : [];
  const right = halves[1] ? halves[1].split(":") : [];
  const missing = halves.length === 2 ? 8 - left.length - right.length : 0;
  const groups = [...left, ...Array(Math.max(0, missing)).fill("0"), ...right];
  if (groups.length !== 8 || groups.some(group => !/^[0-9a-f]{1,4}$/.test(group))) return true;
  const value = groups.reduce((result, group) => (result << 16n) | BigInt(parseInt(group, 16)), 0n);
  const inPrefix = (prefix: bigint, bits: number) => value >> BigInt(128 - bits) === prefix >> BigInt(128 - bits);

  return value <= 1n ||
    inPrefix(0n, 96) || // IPv4-compatible and other special low addresses
    inPrefix(0xffffn << 32n, 96) || // IPv4-mapped addresses
    inPrefix(0x64ff9bn << 96n, 96) || // well-known NAT64
    inPrefix(0x64ff9b0001n << 80n, 48) || // local-use NAT64
    inPrefix(0x100n << 112n, 64) || // discard-only
    inPrefix(0x20010000n << 96n, 23) || // IETF protocol/reserved space
    inPrefix(0x20010db8n << 96n, 32) || // documentation range
    inPrefix(0x20020n << 108n, 16) || // 6to4
    inPrefix(0xfc0n << 116n, 7) || // unique-local
    inPrefix(0xfe8n << 116n, 10) || // link-local
    inPrefix(0xfecn << 116n, 10) || // deprecated site-local
    inPrefix(0xffn << 120n, 8); // multicast
}

export function isPrivateAddress(address: string): boolean {
  const family = net.isIP(address);
  return family === 4 ? isPrivateIpv4(address) : family === 6 ? isPrivateIpv6(address) : true;
}

export function validateAgentUrl(rawUrl: string): URL {
  let parsed: URL;
  try {
    parsed = new URL(rawUrl);
  } catch {
    throw new Error(`Invalid URL: ${rawUrl}`);
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") throw new Error("Only http and https URLs are allowed.");
  if (parsed.username || parsed.password) throw new Error("URLs containing credentials are not allowed.");
  const hostname = parsed.hostname.replace(/^\[|\]$/g, "");
  if (!hostname || hostname.toLowerCase() === "localhost" || hostname.toLowerCase().endsWith(".localhost")) {
    throw new Error("Local and private network addresses are not allowed.");
  }
  if (net.isIP(hostname) && isPrivateAddress(hostname)) {
    throw new Error("Local and private network addresses are not allowed.");
  }
  return parsed;
}

export function selectPublicAddress(addresses: Array<{ address: string; family: number }>): { address: string; family: number } {
  if (addresses.length === 0) throw new Error("Host did not resolve to an address.");
  // Reject mixed answers as well as all-private answers. Pinning a connection to
  // a public result is not enough when the same name can rebind to a private one.
  if (addresses.some(item => isPrivateAddress(item.address))) {
    throw new Error("Host resolves to a local, private, or reserved network address.");
  }
  return addresses[0];
}

async function resolvePublicAddress(hostname: string): Promise<{ address: string; family: number }> {
  if (net.isIP(hostname)) return { address: hostname, family: net.isIP(hostname) };
  const addresses = await dns.lookup(hostname, { all: true, verbatim: true });
  return selectPublicAddress(addresses);
}

function withinDeadline<T>(operation: Promise<T>, timeoutMs: number, signal?: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`Request timed out after ${timeoutMs}ms.`)), timeoutMs);
    const abort = () => reject(signal?.reason instanceof Error ? signal.reason : new Error("Request aborted."));
    if (signal?.aborted) abort();
    else signal?.addEventListener("abort", abort, { once: true });
    operation.then(resolve, reject).finally(() => {
      clearTimeout(timer);
      signal?.removeEventListener("abort", abort);
    });
  });
}

async function requestOnce(url: URL, timeoutMs: number, signal?: AbortSignal): Promise<{ status: number; headers: http.IncomingHttpHeaders; body: string }> {
  const controller = new AbortController();
  const abort = () => controller.abort(signal?.reason);
  if (signal?.aborted) abort();
  else signal?.addEventListener("abort", abort, { once: true });
  const timer = setTimeout(() => controller.abort(new Error(`Request timed out after ${timeoutMs}ms.`)), timeoutMs);
  try {
    const resolved = await withinDeadline(resolvePublicAddress(url.hostname.replace(/^\[|\]$/g, "")), timeoutMs, controller.signal);
    return await new Promise((resolve, reject) => {
      const transport = url.protocol === "https:" ? https : http;
      const request = transport.request(url, {
        method: "GET",
        headers: { "user-agent": "Locagens/1.0 (+local workspace assistant)", accept: "text/*,application/json" },
        lookup(_hostname, _options, callback) {
          callback(null, resolved.address, resolved.family);
        },
        signal: controller.signal,
        timeout: timeoutMs
      }, response => {
        const chunks: Buffer[] = [];
        let total = 0;
        response.on("data", (chunk: Buffer) => {
          total += chunk.length;
          if (total > MAX_BODY_BYTES) {
            response.destroy(new Error(`Response exceeded ${MAX_BODY_BYTES} bytes.`));
            return;
          }
          chunks.push(Buffer.from(chunk));
        });
        response.on("end", () => resolve({
          status: response.statusCode || 0,
          headers: response.headers,
          body: Buffer.concat(chunks).toString("utf-8")
        }));
        response.on("error", reject);
      });
      request.on("timeout", () => request.destroy(new Error(`Request timed out after ${timeoutMs}ms.`)));
      request.on("error", reject);
      request.end();
    });
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener("abort", abort);
  }
}

export async function safeFetchText(rawUrl: string, options: { timeoutMs?: number; signal?: AbortSignal } = {}): Promise<SafeHttpResponse> {
  let current = validateAgentUrl(rawUrl.trim());
  const timeoutMs = options.timeoutMs ?? 30_000;
  const deadline = Date.now() + timeoutMs;
  for (let redirects = 0; redirects <= MAX_REDIRECTS; redirects++) {
    const remainingMs = deadline - Date.now();
    if (remainingMs <= 0) throw new Error(`Request timed out after ${timeoutMs}ms.`);
    const response = await requestOnce(current, remainingMs, options.signal);
    if (response.status >= 300 && response.status < 400 && response.headers.location) {
      if (redirects === MAX_REDIRECTS) throw new Error(`Too many redirects (maximum ${MAX_REDIRECTS}).`);
      current = validateAgentUrl(new URL(response.headers.location, current).toString());
      continue;
    }
    return {
      status: response.status,
      ok: response.status >= 200 && response.status < 300,
      contentType: String(response.headers["content-type"] || ""),
      finalUrl: current.toString(),
      body: response.body
    };
  }
  throw new Error("Request failed.");
}
