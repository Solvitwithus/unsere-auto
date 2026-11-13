"use client";
// Operator screen at the weighbridge PC (served by wb-station on localhost).
//
// Flow, same as the old Java screen but safer:
//   1. Truck drives on. The big display turns green when the weight is STABLE.
//   2. New weighing: choose type, plate, customer/product -> "Capture FIRST weight".
//      (If the truck's empty weight is stored you can tick "single weighing".)
//   3. Truck comes back: click it in "Waiting for second weight" ->
//      "Capture SECOND weight". Net = |first - second|. Ticket prints.
// Weights are taken by the station itself from the live reading, so the
// screen can never submit a typed or stale number (except a supervisor's
// manual entry, if allowed, which is flagged on the ticket).

import { useEffect, useRef, useState } from "react";
import { LoginForm, Status } from "@/components/ui";
import { api, apiText, getToken, kg, printText, setToken, dt, wsUrl, type User } from "@/lib/api";

type Reading = { weight_kg: number; stable: boolean; raw: string };
type ScaleState = { link: string; connected: boolean; receiving: boolean; error?: string | null; reading?: Reading | null };

export default function StationPage() {
  const [status, setStatus] = useState<any>(null);
  const [user, setUser] = useState<User | null>(null);
  const [checked, setChecked] = useState(false);
  useEffect(() => {
    api("/api/station/status")
      .then((s) => {
        setStatus(s);
        if (!s.enrolled) return window.location.replace("/station/setup/");
        if (!getToken()) return setChecked(true);
        api<User>("/api/auth/me").then(setUser).catch(() => setToken(null)).finally(() => setChecked(true));
      })
      .catch(() => window.location.replace("/dashboard/"));
  }, []);
  if (!checked) return <div className="main muted">Loading...</div>;
  if (!user) return <LoginForm title={`Weighbridge ${status?.station_code ?? ""}`} onDone={setUser} />;
  return <Operator user={user} initial={status} />;
}

function useLiveWeight() {
  const [s, setS] = useState<ScaleState | null>(null);
  useEffect(() => {
    let ws: WebSocket | null = null;
    let stop = false;
    const connect = () => {
      ws = new WebSocket(wsUrl("/ws/weight"));
      ws.onmessage = (e) => setS(JSON.parse(e.data));
      ws.onclose = () => !stop && setTimeout(connect, 1000); // station restarted: reconnect
    };
    connect();
    return () => { stop = true; ws?.close(); };
  }, []);
  return s;
}

const TYPES = [["purchase", "Purchase (in)"], ["sale", "Sale (out)"], ["transfer", "Transfer"], ["other", "Other"]];

function Operator({ user, initial }: { user: User; initial: any }) {
  const scale = useLiveWeight();
  const [status, setStatus] = useState<any>(initial);
  const [m, setM] = useState<{ parties: any[]; products: any[]; vehicles: any[] }>({ parties: [], products: [], vehicles: [] });
  const [open, setOpen] = useState<any[]>([]);
  const [recent, setRecent] = useState<any[]>([]);
  const [msg, setMsg] = useState<{ ok?: string; err?: string }>({});
  const [busy, setBusy] = useState(false);
  const blank = { trans_type: "sale", vehicle_plate: "", driver_name: "", party_id: "", party_name: "", product_id: "", source: "", destination: "", reference: "", memo: "", charges: "", manual: "", single: false };
  const [f, setF] = useState(blank);
  const plateRef = useRef<HTMLInputElement>(null);
  const supervisor = user.role === "admin" || user.role === "supervisor";

  const refresh = () => {
    api("/api/tickets?status=open").then(setOpen).catch(() => {});
    api<any[]>("/api/tickets").then((r) => setRecent(r.filter((t) => t.status !== "open").slice(0, 12))).catch(() => {});
    api("/api/station/status").then(setStatus).catch(() => {});
  };
  useEffect(() => {
    api("/api/masters").then(setM).catch(() => {});
    refresh();
    const t = setInterval(refresh, 10000);
    return () => clearInterval(t);
  }, []);

  const r = scale?.reading;
  const live = !!scale?.receiving && !!r;
  const stable = live && r!.stable;
  const vehicle = m.vehicles.find((v) => v.plate === f.vehicle_plate.toUpperCase().replace(/[^A-Z0-9]/g, ""));
  const set = (k: string, v: any) => setF((o) => ({ ...o, [k]: v }));

  async function run(fn: () => Promise<any>, ok: string) {
    setBusy(true);
    setMsg({});
    try {
      const t = await fn();
      setMsg({ ok: `${ok}: ${t.ticket_no} - ${kg(t.status === "complete" ? t.net_weight_kg : t.first_weight_kg)}${t.status === "complete" ? " net" : ""}` });
      refresh();
      return t;
    } catch (e: any) {
      setMsg({ err: e.message });
    } finally {
      setBusy(false);
    }
  }

  async function first() {
    const t = await run(
      () => api("/api/tickets", {
        body: {
          trans_type: f.trans_type, vehicle_plate: f.vehicle_plate, driver_name: f.driver_name,
          party_id: f.party_id || null, party_name: f.party_name, product_id: f.product_id || null,
          source: f.source, destination: f.destination, reference: f.reference, memo: f.memo,
          charges: f.charges === "" ? null : Number(f.charges),
          manual_weight_kg: f.manual === "" ? null : Number(f.manual),
          use_stored_tare: f.single,
        },
      }),
      f.single ? "Ticket complete" : "First weight saved",
    );
    if (t) {
      if (t.status === "complete") await print(t);
      setF(blank);
      plateRef.current?.focus();
    }
  }

  async function second(t: any, manual?: number) {
    const done = await run(() => api(`/api/tickets/${t.id}/second`, { body: manual ? { manual_weight_kg: manual } : {} }), "Second weight saved");
    if (done) await print(done);
  }

  async function print(t: any) {
    try {
      const r = await api(`/api/tickets/${t.id}/print`, { method: "POST" });
      if (r.browser_print) printText(await apiText(`/api/tickets/${t.id}/text`), t.ticket_no);
    } catch (e: any) {
      setMsg({ err: `Saved, but printing failed: ${e.message}` });
    }
  }

  const sync = status?.sync ?? {};
  return (
    <div className="main" style={{ maxWidth: 1400, margin: "0 auto" }}>
      <div className="row" style={{ marginBottom: 12 }}>
        <h1 style={{ margin: 0 }}>{status?.site?.name ?? "Weighbridge"} · {status?.station_code}</h1>
        <div className="spacer" />
        <span className={`badge ${sync.online ? "ok" : "bad"}`} title={sync.error ?? ""}>{sync.online ? "online" : "offline"}</span>
        {sync.pending > 0 && <span className="badge warn">{sync.pending} to sync</span>}
        <span className="muted small">{user.name}</span>
        {supervisor && <a className="btn" href="/station/setup/">Setup</a>}
        <button onClick={() => { api("/api/auth/logout", { method: "POST" }).finally(() => { setToken(null); location.reload(); }); }}>Sign out</button>
      </div>

      <div className={`weight ${!live ? "" : stable ? "stable" : "motion"}`} style={{ marginBottom: 14 }}>
        <div className="row">
          <div>
            <span className="num">{live ? Math.round(r!.weight_kg).toLocaleString("en-KE") : "-----"}</span>
            <span className="unit">kg</span>
          </div>
          <div className="spacer" />
          <div style={{ textAlign: "right" }}>
            <div style={{ fontSize: 26, fontWeight: 700 }}>{!scale ? "CONNECTING" : !scale.connected ? "NO SCALE" : !live ? "NO SIGNAL" : stable ? "STABLE" : "MOVING"}</div>
            <div className="small" style={{ opacity: 0.85 }}>{scale?.link}{scale?.error ? ` · ${scale.error}` : ""}</div>
          </div>
        </div>
      </div>

      {msg.err && <div className="alert">{msg.err}</div>}
      {msg.ok && <div className="notice">{msg.ok}</div>}

      <div className="grid g2">
        <div className="card">
          <h2>New weighing</h2>
          <div className="typebtns" style={{ marginBottom: 10 }}>
            {TYPES.map(([k, l]) => (
              <button key={k} className={f.trans_type === k ? "active" : ""} onClick={() => set("trans_type", k)}>{l}</button>
            ))}
          </div>
          <div className="grid g2" style={{ gap: 8 }}>
            <div className="field">
              <label>Vehicle plate *</label>
              <input ref={plateRef} list="plates" value={f.vehicle_plate} autoFocus
                onChange={(e) => {
                  const plate = e.target.value.toUpperCase();
                  const v = m.vehicles.find((x) => x.plate === plate.replace(/[^A-Z0-9]/g, ""));
                  setF((o) => ({ ...o, vehicle_plate: plate, driver_name: o.driver_name || v?.driver_name || "", party_id: o.party_id || v?.party_id || "" }));
                }} />
              <datalist id="plates">{m.vehicles.map((v) => <option key={v.id} value={v.plate} />)}</datalist>
              {vehicle?.stored_tare_kg && <div className="small muted">Stored empty weight: {kg(vehicle.stored_tare_kg)}</div>}
            </div>
            <div className="field"><label>Driver</label><input value={f.driver_name} onChange={(e) => set("driver_name", e.target.value)} /></div>
            <div className="field">
              <label>{f.trans_type === "purchase" ? "Supplier" : "Customer"}</label>
              <select value={f.party_id} onChange={(e) => set("party_id", e.target.value)}>
                <option value="">- type a name below / none -</option>
                {m.parties.filter((p) => f.trans_type === "purchase" ? p.kind !== "customer" : p.kind !== "supplier").map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
              </select>
              {!f.party_id && <input style={{ marginTop: 6 }} placeholder="or name (not in the list)" value={f.party_name} onChange={(e) => set("party_name", e.target.value)} />}
            </div>
            <div className="field">
              <label>Product</label>
              <select value={f.product_id} onChange={(e) => set("product_id", e.target.value)}>
                <option value="">-</option>
                {m.products.filter((p) => p.code !== "FEE").map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
              </select>
            </div>
            <div className="field"><label>From</label><input value={f.source} placeholder={f.trans_type === "sale" ? status?.site?.name : ""} onChange={(e) => set("source", e.target.value)} /></div>
            <div className="field"><label>To</label><input value={f.destination} placeholder={f.trans_type === "purchase" ? status?.site?.name : ""} onChange={(e) => set("destination", e.target.value)} /></div>
            <div className="field"><label>Reference / order no.</label><input value={f.reference} onChange={(e) => set("reference", e.target.value)} /></div>
            <div className="field"><label>Weighing charges</label><input type="number" value={f.charges} onChange={(e) => set("charges", e.target.value)} /></div>
            <div className="field" style={{ gridColumn: "1/-1" }}><label>Memo</label><input value={f.memo} onChange={(e) => set("memo", e.target.value)} /></div>
          </div>
          {vehicle?.stored_tare_kg && (
            <label className="check" style={{ marginBottom: 8 }}>
              <input type="checkbox" checked={f.single} onChange={(e) => set("single", e.target.checked)} />
              Single weighing: use the stored empty weight ({kg(vehicle.stored_tare_kg)}) and finish now
            </label>
          )}
          {status?.allow_manual && supervisor && (
            <div className="field">
              <label>Manual weight (only if the scale is down - flagged on the ticket)</label>
              <input type="number" value={f.manual} onChange={(e) => set("manual", e.target.value)} />
            </div>
          )}
          <button className="primary big" disabled={busy || !f.vehicle_plate || (!stable && !f.manual)} onClick={first}>
            {f.single ? "Capture weight & finish" : "Capture FIRST weight"} {stable ? `(${kg(r!.weight_kg)})` : ""}
          </button>
          {!stable && !f.manual && <div className="small muted" style={{ marginTop: 6 }}>The button unlocks when the weight is stable.</div>}
        </div>

        <div>
          <div className="card" style={{ marginBottom: 14 }}>
            <h2>Waiting for second weight ({open.length})</h2>
            <table>
              <tbody>
                {open.map((t) => (
                  <tr key={t.id}>
                    <td><b className="mono">{t.vehicle_plate}</b><div className="small muted">{t.ticket_no} · {t.trans_type} · {t.party_name}</div></td>
                    <td className="right">{kg(t.first_weight_kg)}<div className="small muted">{dt(t.first_weight_at)}</div></td>
                    <td className="right">
                      <button className="primary" disabled={busy || !stable} onClick={() => second(t)}>SECOND weight</button>
                      {status?.allow_manual && supervisor && (
                        <button style={{ marginLeft: 4 }} onClick={() => { const w = prompt("Manual second weight (kg)"); if (w) second(t, Number(w)); }}>✎</button>
                      )}
                    </td>
                  </tr>
                ))}
                {!open.length && <tr><td className="muted">No trucks waiting.</td></tr>}
              </tbody>
            </table>
          </div>
          <div className="card">
            <h2>Recent tickets</h2>
            <table>
              <tbody>
                {recent.map((t) => (
                  <tr key={t.id}>
                    <td className="mono">{t.ticket_no}</td>
                    <td className="mono">{t.vehicle_plate}</td>
                    <td className="right"><b>{kg(t.net_weight_kg)}</b></td>
                    <td><Status s={t.status} /></td>
                    <td className="right"><button onClick={() => print(t)}>Print</button></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </div>
  );
}
