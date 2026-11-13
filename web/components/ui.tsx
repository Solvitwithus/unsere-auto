"use client";
// Small shared UI pieces: login form, dashboard shell (nav + auth guard), modal, status badges.

import { useEffect, useState, type ReactNode } from "react";
import { api, setToken, getToken, type User } from "@/lib/api";

/** Username/password form. `onDone` gets the signed-in user. Works for server and station. */
export function LoginForm({ onDone, title }: { onDone: (u: User) => void; title: string }) {
  const [username, setU] = useState("");
  const [password, setP] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setErr("");
    try {
      const r = await api<{ token: string; user: User }>("/api/auth/login", { body: { username, password } });
      setToken(r.token);
      onDone(r.user);
    } catch (e: any) {
      setErr(e.message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <div style={{ display: "grid", placeItems: "center", minHeight: "80vh", padding: 16 }}>
      <form className="card" style={{ width: "min(380px,100%)" }} onSubmit={submit}>
        <h1>{title}</h1>
        {err && <div className="alert">{err}</div>}
        <div className="field">
          <label>Username</label>
          <input value={username} onChange={(e) => setU(e.target.value)} autoFocus autoComplete="username" />
        </div>
        <div className="field">
          <label>Password</label>
          <input type="password" value={password} onChange={(e) => setP(e.target.value)} autoComplete="current-password" />
        </div>
        <button className="primary big" disabled={busy || !username || !password}>
          {busy ? "Signing in..." : "Sign in"}
        </button>
      </form>
    </div>
  );
}

const NAV = [
  { href: "/dashboard/", label: "Dashboard" },
  { href: "/tickets/", label: "Tickets" },
  { href: "/sales/", label: "Sales (eTIMS)" },
  { href: "/masters/", label: "Customers & items" },
  { href: "/admin/", label: "Sites, stations, users", admin: true },
];

/**
 * Wraps every dashboard page: checks the session (shows the login form if
 * needed), sends station installs to the operator screen, and draws the nav.
 * The page receives the signed-in user.
 */
export function Shell({ children }: { children: (u: User) => ReactNode }) {
  const [user, setUser] = useState<User | null>(null);
  const [checked, setChecked] = useState(false);
  const [path, setPath] = useState("");

  useEffect(() => {
    setPath(window.location.pathname);
    // The station serves this same UI: if this is a station, go to the operator screen.
    api("/api/station/status")
      .then(() => window.location.replace("/station/"))
      .catch(() => {
        if (!getToken()) return setChecked(true);
        api<User>("/api/auth/me")
          .then(setUser)
          .catch(() => setToken(null))
          .finally(() => setChecked(true));
      });
  }, []);

  if (!checked) return <div className="main muted">Loading...</div>;
  if (!user) return <LoginForm title="Weighbridge" onDone={setUser} />;
  return (
    <div className="shell">
      <nav className="nav">
        <div className="brand">⚖ Weighbridge</div>
        {NAV.filter((n) => !n.admin || user.role === "admin" || user.role === "supervisor").map((n) => (
          <a key={n.href} href={n.href} className={path.startsWith(n.href) ? "active" : ""}>
            {n.label}
          </a>
        ))}
        <div className="spacer" />
        <div className="small muted" style={{ padding: "8px 10px" }}>
          {user.name || user.username}
          <br />
          {user.role}
        </div>
        <a
          href="#"
          onClick={(e) => {
            e.preventDefault();
            setToken(null);
            window.location.href = "/dashboard/";
          }}
        >
          Sign out
        </a>
      </nav>
      <main className="main">{children(user)}</main>
    </div>
  );
}

export function Modal({ title, onClose, children, wide }: { title: string; onClose: () => void; children: ReactNode; wide?: boolean }) {
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" style={wide ? { width: "min(1000px,100%)" } : undefined}>
        <div className="row" style={{ marginBottom: 12 }}>
          <h2 style={{ margin: 0 }}>{title}</h2>
          <div className="spacer" />
          <button onClick={onClose}>Close</button>
        </div>
        {children}
      </div>
    </div>
  );
}

/** Coloured badge for ticket and eTIMS statuses. */
export function Status({ s }: { s?: string | null }) {
  if (!s) return <span className="muted">-</span>;
  const cls: Record<string, string> = {
    complete: "ok", open: "warn", void: "bad",
    SIGNED: "ok", SUBMITTED: "info", PENDING: "warn", FAILED: "bad", DISABLED: "", APPROVED: "ok", CANCELED: "bad",
  };
  return <span className={`badge ${cls[s] ?? ""}`}>{s}</span>;
}
