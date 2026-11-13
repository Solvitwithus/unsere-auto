// Tiny API client shared by every page.
//
// The same UI build is served by two programs:
//   * wb-server  (central)  -> dashboard pages, talks to /api/... on the server
//   * wb-station (local PC) -> operator screen, talks to /api/... on the station
// Both serve the UI and the API from the same origin, so paths are relative.
// During development set NEXT_PUBLIC_API_BASE=http://127.0.0.1:8091 (and
// CORS_ORIGINS=http://localhost:3000 on the server).

export const API_BASE = process.env.NEXT_PUBLIC_API_BASE ?? "";
const TOKEN_KEY = "wb_token";

// localStorage can be unavailable (private mode, locked-down kiosks): never let that crash the page.
export function getToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export function setToken(t: string | null) {
  try {
    if (t) localStorage.setItem(TOKEN_KEY, t);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* ignore */
  }
}

export class ApiError extends Error {
  constructor(message: string, public status: number) {
    super(message);
  }
}

/** Call the API. Throws ApiError with the server's message on failure. */
export async function api<T = any>(path: string, opts: { method?: string; body?: unknown } = {}): Promise<T> {
  const headers: Record<string, string> = {};
  const token = getToken();
  if (token) headers["authorization"] = `Bearer ${token}`;
  if (opts.body !== undefined) headers["content-type"] = "application/json";
  let res: Response;
  try {
    res = await fetch(API_BASE + path, {
      method: opts.method ?? (opts.body !== undefined ? "POST" : "GET"),
      headers,
      body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
    });
  } catch {
    throw new ApiError("Cannot reach the server. Check the network.", 0);
  }
  const text = await res.text();
  let data: any = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = text;
  }
  if (!res.ok) {
    if (res.status === 401) setToken(null);
    throw new ApiError((data && data.error) || (typeof data === "string" && data.slice(0, 300)) || `HTTP ${res.status}`, res.status);
  }
  return data as T;
}

/** Fetch plain text (ticket/receipt printouts). */
export async function apiText(path: string): Promise<string> {
  const token = getToken();
  const res = await fetch(API_BASE + path, { headers: token ? { authorization: `Bearer ${token}` } : {} });
  if (!res.ok) throw new ApiError(`HTTP ${res.status}`, res.status);
  return res.text();
}

/** Download a file that needs the auth header (CSV export). */
export async function download(path: string, filename: string) {
  const token = getToken();
  const res = await fetch(API_BASE + path, { headers: token ? { authorization: `Bearer ${token}` } : {} });
  if (!res.ok) throw new ApiError(`HTTP ${res.status}`, res.status);
  const url = URL.createObjectURL(await res.blob());
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

/** WebSocket URL on the same host as the API. */
export function wsUrl(path: string): string {
  const base = API_BASE || window.location.origin;
  return base.replace(/^http/, "ws") + path;
}

/** Open a print window with monospace ticket text (works with any printer). */
export function printText(text: string, title = "Ticket") {
  const w = window.open("", "_blank", "width=420,height=640");
  if (!w) return alert("Allow pop-ups to print");
  const esc = text.replace(/&/g, "&amp;").replace(/</g, "&lt;");
  w.document.write(
    `<html><head><title>${title}</title><style>body{margin:0;padding:8px}pre{font:12px/1.35 "Courier New",monospace;margin:0}@page{margin:4mm}</style></head><body><pre>${esc}</pre><script>window.onload=()=>{window.print();setTimeout(()=>window.close(),300)}<\/script></body></html>`,
  );
  w.document.close();
}

// ---- formatting ----

export const kg = (n: number | null | undefined) => (n == null ? "-" : `${Math.round(n).toLocaleString("en-KE")} kg`);
export const money = (n: number | null | undefined) =>
  n == null ? "-" : n.toLocaleString("en-KE", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
export const dt = (s: string | null | undefined) =>
  s ? new Date(s).toLocaleString("en-KE", { day: "2-digit", month: "short", hour: "2-digit", minute: "2-digit" }) : "-";
export const today = () => new Date().toISOString().slice(0, 10);
export const daysAgo = (n: number) => new Date(Date.now() - n * 86400000).toISOString().slice(0, 10);

export type User = { id: string; username: string; name: string; role: string; site_id?: string | null };
export const canManage = (u: User | null) => !!u && (u.role === "admin" || u.role === "supervisor");
