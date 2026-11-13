"use client";
// Station setup (on the weighbridge PC):
//   1. Connect to the server: paste the server address and the station key
//      from the dashboard (Sites, stations, users > Stations > Add).
//   2. Scale: Auto-detect finds the port, baud rate and protocol by itself.
//      For an unknown indicator use Learn: type the number on its display.
//   3. Printer: browser printing works with any printer; or a receipt printer.
// Step 1 is open on a fresh install; after that a supervisor must sign in.

import { useEffect, useState } from "react";
import { LoginForm } from "@/components/ui";
import { ScaleEditor, defaultScale, type ScaleConfig } from "@/components/scale";
import { api, getToken, kg, setToken, type User } from "@/lib/api";

export default function SetupPage() {
  const [status, setStatus] = useState<any>(null);
  const [user, setUser] = useState<User | null>(null);
  const [checked, setChecked] = useState(false);
  const load = () => api("/api/station/status").then(setStatus);
  useEffect(() => {
    load().then(() => {
      if (!getToken()) return setChecked(true);
      api<User>("/api/auth/me").then(setUser).catch(() => setToken(null)).finally(() => setChecked(true));
    }).catch(() => setChecked(true));
  }, []);
  if (!checked || !status) return <div className="main muted">Loading...</div>;
  if (!status.enrolled) return <Enrol onDone={() => location.replace("/station/")} />;
  if (!user) return <LoginForm title="Station setup - supervisor sign in" onDone={setUser} />;
  if (!["admin", "supervisor"].includes(user.role)) return <div className="main"><div className="alert">A supervisor must open setup.</div><a href="/station/">Back</a></div>;
  return (
    <div className="main" style={{ maxWidth: 1000, margin: "0 auto" }}>
      <div className="row" style={{ marginBottom: 12 }}>
        <h1 style={{ margin: 0 }}>Station setup · {status.station_code}</h1>
        <div className="spacer" />
        <a className="btn" href="/station/">Back to weighing</a>
      </div>
      <ScaleSetup />
      <PrinterSetup status={status} />
      <div className="card" style={{ marginTop: 14 }}>
        <h2>Server connection</h2>
        <p className="small">Connected to <span className="mono">{status.server_url}</span> as station <b>{status.station_code}</b> ({status.site?.name}).</p>
        <Enrol inline onDone={load} />
      </div>
    </div>
  );
}

function Enrol({ onDone, inline }: { onDone: () => void; inline?: boolean }) {
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const form = (
    <>
      {err && <div className="alert">{err}</div>}
      <div className="field"><label>Server address</label><input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://weighbridge.example.com" /></div>
      <div className="field"><label>Station key</label><input value={key} onChange={(e) => setKey(e.target.value)} placeholder="wbk_..." className="mono" /></div>
      <button className="primary" disabled={busy || !url || !key} onClick={async () => {
        setBusy(true); setErr("");
        try { await api("/api/station/setup", { body: { server_url: url, station_key: key } }); onDone(); }
        catch (e: any) { setErr(e.message); } finally { setBusy(false); }
      }}>{busy ? "Connecting..." : inline ? "Change server / key" : "Connect"}</button>
    </>
  );
  if (inline) return form;
  return (
    <div style={{ display: "grid", placeItems: "center", minHeight: "80vh", padding: 16 }}>
      <div className="card" style={{ width: "min(520px,100%)" }}>
        <h1>Set up this weighbridge</h1>
        <p className="muted small">In the dashboard, go to Sites, stations, users &gt; Stations &gt; Add. Copy the key it shows here.</p>
        {form}
      </div>
    </div>
  );
}

function ScaleSetup() {
  const [cfg, setCfg] = useState<ScaleConfig | null>(null);
  const [ports, setPorts] = useState<string[]>([]);
  const [cands, setCands] = useState<any[] | null>(null);
  const [tcp, setTcp] = useState("");
  const [busy, setBusy] = useState("");
  const [msg, setMsg] = useState<{ ok?: string; err?: string }>({});
  const [learnW, setLearnW] = useState("");
  const [raw, setRaw] = useState("");
  useEffect(() => {
    api("/api/scale/config").then((c) => setCfg(c ?? defaultScale()));
    api<string[]>("/api/scale/ports").then(setPorts).catch(() => {});
  }, []);
  const save = async (c: ScaleConfig) => {
    setBusy("save"); setMsg({});
    try { await api("/api/scale/config", { method: "PUT", body: c }); setCfg(c); setMsg({ ok: "Saved. The scale reader restarted with these settings." }); }
    catch (e: any) { setMsg({ err: e.message }); } finally { setBusy(""); }
  };
  return (
    <div className="card">
      <h2>Scale</h2>
      {msg.err && <div className="alert">{msg.err}</div>}
      {msg.ok && <div className="notice">{msg.ok}</div>}
      <div className="row" style={{ marginBottom: 12 }}>
        <button className="primary" disabled={!!busy} onClick={async () => {
          setBusy("detect"); setMsg({}); setCands(null);
          try { const r = await api("/api/scale/detect", { body: tcp ? { tcp } : {} }); setCands(r.candidates); }
          catch (e: any) { setMsg({ err: e.message }); } finally { setBusy(""); }
        }}>{busy === "detect" ? "Listening on every port... (up to a minute)" : "Auto-detect"}</button>
        <input placeholder="or network indicator host:port" value={tcp} onChange={(e) => setTcp(e.target.value)} style={{ width: 260 }} />
        <span className="small muted">Ports found: {ports.join(", ") || "none"}</span>
      </div>
      {cands && (
        <div className="tablewrap" style={{ marginBottom: 12 }}>
          {!cands.length && <div className="alert">Nothing recognised. Check the cable, that the indicator's serial output is switched on (continuous mode), or use Learn below.</div>}
          {!!cands.length && <p className="small">Pick the line whose weights match the indicator display:</p>}
          <table>
            <tbody>
              {cands.map((c, i) => (
                <tr key={i}>
                  <td><b>{c.transport.port ?? `${c.transport.host}:${c.transport.port}`}</b> {c.transport.baud} {c.transport.framing}</td>
                  <td>{c.profile.name}</td>
                  <td>{c.sample_weights.map((w: number) => kg(w)).join(", ")}</td>
                  <td className="small muted">{Math.round(c.score * 100)}%</td>
                  <td><button onClick={() => save({ ...(cfg ?? defaultScale()), transport: c.transport, profile_id: c.profile.id, profile: undefined })}>Use this</button></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <ScaleEditor value={cfg} onChange={setCfg} ports={ports} />
      <div className="row" style={{ marginTop: 10 }}>
        <button className="primary" disabled={!!busy || !cfg} onClick={() => cfg && save(cfg)}>Save scale settings</button>
        <div className="spacer" />
        <button onClick={() => api("/api/scale/raw").then((r) => setRaw(r.raw || "(nothing received)"))}>Show raw data</button>
      </div>
      {raw && <pre className="ticket" style={{ marginTop: 10, whiteSpace: "pre-wrap", wordBreak: "break-all" }}>{raw}</pre>}
      <div className="card" style={{ marginTop: 12, background: "var(--bg)" }}>
        <h3>Learn an unknown indicator</h3>
        <p className="small muted">Set the port and baud above and save. Put a steady load on the deck, type exactly the number shown on the indicator, and press Learn.</p>
        <div className="row">
          <input type="number" placeholder="Displayed weight, kg" value={learnW} onChange={(e) => setLearnW(e.target.value)} style={{ width: 200 }} />
          <button disabled={!!busy || !learnW} onClick={async () => {
            setBusy("learn"); setMsg({});
            try { const r = await api("/api/scale/learn", { body: { weight_kg: Number(learnW) } }); await save({ ...(cfg ?? defaultScale()), profile: r.profile, profile_id: null }); setMsg({ ok: `Learned: ${r.profile.description}. Saved.` }); }
            catch (e: any) { setMsg({ err: e.message }); } finally { setBusy(""); }
          }}>{busy === "learn" ? "Listening..." : "Learn"}</button>
        </div>
      </div>
    </div>
  );
}

function PrinterSetup({ status }: { status: any }) {
  const [p, setP] = useState<any>(status.printer ?? { kind: "browser" });
  const [auto, setAuto] = useState<boolean>(!!status.auto_print);
  const [msg, setMsg] = useState("");
  return (
    <div className="card" style={{ marginTop: 14 }}>
      <h2>Ticket printer</h2>
      <div className="grid g2">
        <div className="field">
          <label>Printer type</label>
          <select value={p.kind} onChange={(e) => setP({ kind: e.target.value, ...(e.target.value === "tcp" ? { port: 9100 } : e.target.value === "serial" ? { baud: 9600 } : {}) })}>
            <option value="browser">Any printer (print dialog / Windows default printer)</option>
            <option value="tcp">Network receipt printer (ESC/POS, port 9100)</option>
            <option value="serial">Serial / USB receipt printer (ESC/POS)</option>
            <option value="file">Device or file path</option>
            <option value="command">Print command (e.g. lp -d PRINTER)</option>
          </select>
        </div>
        {p.kind === "tcp" && (<><div className="field"><label>Printer IP</label><input value={p.host ?? ""} onChange={(e) => setP({ ...p, host: e.target.value })} /></div><div className="field"><label>Port</label><input type="number" value={p.port} onChange={(e) => setP({ ...p, port: Number(e.target.value) })} /></div></>)}
        {p.kind === "serial" && (<><div className="field"><label>Port</label><input value={p.port ?? ""} onChange={(e) => setP({ ...p, port: e.target.value })} placeholder="COM5" /></div><div className="field"><label>Baud</label><input type="number" value={p.baud} onChange={(e) => setP({ ...p, baud: Number(e.target.value) })} /></div></>)}
        {p.kind === "file" && <div className="field"><label>Path</label><input value={p.path ?? ""} onChange={(e) => setP({ ...p, path: e.target.value })} placeholder="/dev/usb/lp0" /></div>}
        {p.kind === "command" && <div className="field"><label>Command</label><input value={p.command ?? ""} onChange={(e) => setP({ ...p, command: e.target.value })} /></div>}
      </div>
      <label className="check"><input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} /> Print automatically when a ticket is completed</label>
      <div className="row" style={{ marginTop: 10 }}>
        <button className="primary" onClick={() => api("/api/station/printer", { method: "PUT", body: { printer: p, auto_print: auto } }).then(() => setMsg("Saved")).catch((e) => setMsg(e.message))}>Save printer</button>
        <span className="small muted">{msg}</span>
      </div>
    </div>
  );
}
