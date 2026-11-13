"use client";
// Sites (clients/branches), stations (weighbridge PCs and their scale
// settings), users and the audit log. A new client = a new site + station:
// no code changes, which is what replaced the seven copied Java screens.

import { useEffect, useState } from "react";
import { Crud } from "@/components/crud";
import { ScaleEditor, defaultScale } from "@/components/scale";
import { Modal, Shell } from "@/components/ui";
import { api, dt, kg, type User } from "@/lib/api";

export default function AdminPage() {
  return <Shell>{(u) => <Admin user={u} />}</Shell>;
}

function Admin({ user }: { user: User }) {
  const [tab, setTab] = useState("sites");
  const [sites, setSites] = useState<any[]>([]);
  const [key, setKey] = useState<string | null>(null);
  useEffect(() => {
    api<any[]>("/api/sites").then(setSites).catch(() => {});
  }, [tab]);
  const admin = user.role === "admin";
  const siteOpts = sites.map((s) => ({ value: s.id, label: `${s.code} - ${s.name}` }));
  return (
    <>
      <h1>Sites, stations, users</h1>
      {!admin && <div className="notice">Only admins can change these. You can view them.</div>}
      <div className="tabs">
        {[["sites", "Sites (clients)"], ["stations", "Stations (weighbridges)"], ["users", "Users"], ["audit", "Audit log"]].map(([k, l]) => (
          <button key={k} className={tab === k ? "active" : ""} onClick={() => setTab(k)}>{l}</button>
        ))}
      </div>

      {tab === "sites" && (
        <Crud
          title="Site"
          endpoint="/api/sites"
          query=""
          canEdit={admin}
          fields={[
            { key: "code", label: "Code", help: "Short, e.g. SPENZA. Prefixes invoice numbers." },
            { key: "name", label: "Company / site name", help: "Printed on tickets; default source on sales and destination on purchases" },
            { key: "kra_pin", label: "KRA PIN" },
            { key: "phone", label: "Phone" },
            { key: "address", label: "Address", formOnly: true },
            { key: "ticket_footer", label: "Ticket footer", formOnly: true, default: "Thank you" },
            { key: "weighing_fee", label: "Default weighing fee", type: "number", formOnly: true, default: 0 },
            { key: "prices_include_tax", label: "Prices include VAT", type: "bool", formOnly: true, default: true },
            { key: "auto_invoice", label: "Invoice sale tickets automatically", type: "bool", default: false, help: "When a 'sale' ticket with a priced product is completed, create the invoice (and send it to eTIMS) without anyone pressing a button." },
          ]}
        />
      )}

      {tab === "stations" && (
        <Crud
          title="Station"
          endpoint="/api/stations"
          query=""
          canEdit={admin}
          afterSave={(res, isNew) => isNew && res?.key && setKey(res.key)}
          fields={[
            { key: "site_id", label: "Site", type: "select", options: [{ value: "", label: "Choose..." }, ...siteOpts], render: (r) => r.site_name },
            { key: "code", label: "Code", help: "Short, e.g. SPZ1. Prefixes ticket numbers (SPZ1-000123). Can't be changed later." },
            { key: "name", label: "Name" },
            {
              key: "last_seen_at", label: "Last seen", tableOnly: true,
              render: (r) => {
                const on = r.last_seen_at && Date.now() - new Date(r.last_seen_at).getTime() < 60000;
                return <span>{on ? <span className="badge ok">online</span> : <span className="badge bad">offline</span>} <span className="small muted">{dt(r.last_seen_at)}</span></span>;
              },
            },
            {
              key: "scale_status", label: "Scale", tableOnly: true,
              render: (r) => {
                const s = r.last_status?.scale;
                return s ? <span className="small">{s.link} {s.receiving ? `· ${kg(s.reading?.weight_kg)}` : "· no data"}</span> : <span className="muted">-</span>;
              },
            },
            { key: "tickets_today", label: "Tickets 24h", tableOnly: true },
            {
              key: "scale_config", label: "Scale settings (sent to the station on its next sync)", type: "custom", formOnly: true, default: defaultScale(),
              editor: (v, set) => <ScaleEditor value={v} onChange={set} />,
            },
            { key: "active", label: "Active", type: "bool" },
          ]}
          extraActions={(r) =>
            admin && (
              <button
                className="small"
                onClick={async () => {
                  if (!confirm(`Make a new key for ${r.code}? The station stops syncing until the new key is entered there.`)) return;
                  const res = await api(`/api/stations/${r.id}/rotate-key`, { method: "POST" });
                  setKey(res.key);
                }}
              >
                New key
              </button>
            )
          }
        />
      )}

      {tab === "users" && (
        <Crud
          title="User"
          endpoint="/api/users"
          query=""
          canEdit={admin}
          fields={[
            { key: "username", label: "Username" },
            { key: "full_name", label: "Full name" },
            { key: "password", label: "Password", type: "password", formOnly: true, help: "At least 8 characters. Leave blank to keep the current one." },
            {
              key: "role", label: "Role", type: "select", default: "operator",
              options: [
                { value: "operator", label: "Operator - weighs trucks at a station" },
                { value: "supervisor", label: "Supervisor - also voids, manual weights, masters, invoices" },
                { value: "viewer", label: "Viewer - dashboard and reports only" },
                { value: "admin", label: "Admin - everything" },
              ],
            },
            { key: "site_id", label: "Site", type: "select", options: [{ value: "", label: "All sites" }, ...siteOpts], render: (r) => r.site_name ?? "All" },
            { key: "active", label: "Active", type: "bool" },
          ]}
        />
      )}

      {tab === "audit" && <Audit />}

      {key && (
        <Modal title="Station key" onClose={() => setKey(null)}>
          <p>Enter this key in the station's setup screen. <b>It is shown only once.</b></p>
          <pre className="ticket" style={{ whiteSpace: "pre-wrap", wordBreak: "break-all" }}>{key}</pre>
          <button onClick={() => navigator.clipboard?.writeText(key)}>Copy</button>
        </Modal>
      )}
    </>
  );
}

function Audit() {
  const [rows, setRows] = useState<any[]>([]);
  useEffect(() => {
    api<any[]>("/api/audit").then(setRows).catch(() => {});
  }, []);
  return (
    <div className="card tablewrap" style={{ padding: 0 }}>
      <table>
        <thead><tr><th>When</th><th>Who</th><th>Action</th><th>What</th><th>Details</th></tr></thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.id}>
              <td>{dt(r.at)}</td><td>{r.actor}</td><td>{r.action}</td><td>{r.entity}</td>
              <td className="small mono" style={{ whiteSpace: "normal" }}>{JSON.stringify(r.details)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
