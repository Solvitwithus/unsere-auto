"use client";
// Tickets from every station: search, filter, export, view, void, invoice.

import { useEffect, useState } from "react";
import { Modal, Shell, Status } from "@/components/ui";
import { api, apiText, canManage, daysAgo, download, dt, kg, money, printText, today, type User } from "@/lib/api";

export default function TicketsPage() {
  return <Shell>{(u) => <Tickets user={u} />}</Shell>;
}

function Tickets({ user }: { user: User }) {
  const [f, setF] = useState({ from: daysAgo(6), to: today(), status: "", trans_type: "", q: "" });
  const [data, setData] = useState<{ rows: any[]; total: number }>({ rows: [], total: 0 });
  const [err, setErr] = useState("");
  const [open, setOpen] = useState<any | null>(null);
  const qs = () => new URLSearchParams(Object.entries(f).filter(([, v]) => v) as [string, string][]).toString();
  const load = (more = false) =>
    api(`/api/tickets?${qs()}&limit=100&offset=${more ? data.rows.length : 0}`)
      .then((r) => setData(more ? { rows: [...data.rows, ...r.rows], total: r.total } : r))
      .catch((e) => setErr(e.message));
  useEffect(() => {
    const t = setTimeout(() => load(), 250);
    return () => clearTimeout(t);
  }, [f]);
  const set = (k: string, v: string) => setF((o) => ({ ...o, [k]: v }));

  return (
    <>
      <div className="row" style={{ marginBottom: 12 }}>
        <h1 style={{ margin: 0 }}>Tickets</h1>
        <span className="muted">{data.total.toLocaleString()} found</span>
        <div className="spacer" />
        <button onClick={() => download(`/api/tickets/export.csv?${qs()}`, `tickets-${f.from}-${f.to}.csv`).catch((e) => setErr(e.message))}>Export CSV</button>
      </div>
      <div className="row card" style={{ marginBottom: 12 }}>
        <input type="date" value={f.from} onChange={(e) => set("from", e.target.value)} style={{ width: 150 }} />
        <input type="date" value={f.to} onChange={(e) => set("to", e.target.value)} style={{ width: 150 }} />
        <select value={f.status} onChange={(e) => set("status", e.target.value)} style={{ width: 140 }}>
          <option value="">Any status</option>
          <option value="open">Open</option>
          <option value="complete">Complete</option>
          <option value="void">Void</option>
        </select>
        <select value={f.trans_type} onChange={(e) => set("trans_type", e.target.value)} style={{ width: 140 }}>
          <option value="">Any type</option>
          <option value="purchase">Purchase</option>
          <option value="sale">Sale</option>
          <option value="transfer">Transfer</option>
          <option value="other">Other</option>
        </select>
        <input placeholder="Ticket, plate, customer, product, driver" value={f.q} onChange={(e) => set("q", e.target.value)} style={{ flex: 1, minWidth: 200 }} />
      </div>
      {err && <div className="alert">{err}</div>}
      <div className="card tablewrap" style={{ padding: 0 }}>
        <table>
          <thead>
            <tr>
              <th>Ticket</th><th>Date</th><th>Type</th><th>Vehicle</th><th>Customer / supplier</th><th>Product</th>
              <th className="right">1st</th><th className="right">2nd</th><th className="right">Net</th><th>Status</th><th>Invoice</th>
            </tr>
          </thead>
          <tbody>
            {data.rows.map((t) => (
              <tr key={t.id} className="click" onClick={() => setOpen(t)}>
                <td className="mono">{t.ticket_no}</td>
                <td>{dt(t.created_at)}</td>
                <td>{t.trans_type}</td>
                <td className="mono">{t.vehicle_plate}</td>
                <td>{t.party_name}</td>
                <td>{t.product_name}</td>
                <td className="right">{kg(t.first_weight_kg)}{t.first_manual ? " ✎" : ""}</td>
                <td className="right">{kg(t.second_weight_kg)}{t.second_manual ? " ✎" : ""}</td>
                <td className="right"><b>{kg(t.net_weight_kg)}</b></td>
                <td><Status s={t.status} /></td>
                <td>{t.sale_no ? <span className="small">{t.sale_no} <Status s={t.etims_status} /></span> : ""}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {data.rows.length < data.total && (
        <div style={{ marginTop: 10 }}>
          <button onClick={() => load(true)}>Load more</button>
        </div>
      )}
      <p className="small muted">✎ = weight typed by a supervisor instead of read from the scale.</p>
      {open && <TicketModal t={open} user={user} onClose={() => setOpen(null)} onChanged={() => { setOpen(null); load(); }} />}
    </>
  );
}

function TicketModal({ t, user, onClose, onChanged }: { t: any; user: User; onClose: () => void; onChanged: () => void }) {
  const [err, setErr] = useState("");
  const [pay, setPay] = useState("Cash");
  const [busy, setBusy] = useState(false);
  const act = async (fn: () => Promise<any>) => {
    setBusy(true);
    setErr("");
    try {
      await fn();
      onChanged();
    } catch (e: any) {
      setErr(e.message);
    } finally {
      setBusy(false);
    }
  };
  const rows: [string, React.ReactNode][] = [
    ["Type", t.trans_type], ["Status", <Status key="s" s={t.status} />], ["Site / station", `${t.site_name ?? ""} / ${t.station_code ?? ""}`],
    ["Vehicle", t.vehicle_plate], ["Driver", t.driver_name], ["Customer / supplier", t.party_name], ["Product", t.product_name],
    ["From", t.source], ["To", t.destination], ["Reference", t.reference], ["Memo", t.memo],
    ["First weight", `${kg(t.first_weight_kg)} at ${dt(t.first_weight_at)}${t.first_manual ? " (typed)" : ""}`],
    ["Second weight", t.second_weight_kg != null ? `${kg(t.second_weight_kg)} at ${dt(t.second_weight_at)}${t.second_manual ? " (typed)" : ""}` : "-"],
    ["Net", <b key="n">{kg(t.net_weight_kg)}</b>], ["Charges", money(t.charges)], ["Operator", t.operator_name],
    ["Raw indicator data", <span key="r" className="mono small">{t.first_raw} | {t.second_raw}</span>],
  ];
  if (t.status === "void") rows.push(["Void reason", t.void_reason]);
  return (
    <Modal title={`Ticket ${t.ticket_no}`} onClose={onClose}>
      {err && <div className="alert">{err}</div>}
      <table>
        <tbody>
          {rows.filter(([, v]) => v !== "" && v != null).map(([k, v]) => (
            <tr key={k}><td className="muted">{k}</td><td style={{ whiteSpace: "normal" }}>{v}</td></tr>
          ))}
        </tbody>
      </table>
      <div className="row" style={{ marginTop: 14 }}>
        <button onClick={() => apiText(`/api/tickets/${t.id}/text`).then((x) => printText(x, t.ticket_no))}>Print copy</button>
        {t.sale_id && <a className="btn" href={`/sales/?open=${t.sale_id}`}>Open invoice {t.sale_no}</a>}
        <div className="spacer" />
        {!t.sale_id && t.status === "complete" && (
          <>
            <select value={pay} onChange={(e) => setPay(e.target.value)} style={{ width: 130 }}>
              {["Cash", "Credit", "Mobile Money", "Bank Transfer", "Cheque", "Card"].map((p) => <option key={p}>{p}</option>)}
            </select>
            <button className="primary" disabled={busy} onClick={() => act(() => api(`/api/tickets/${t.id}/sale`, { body: { payment_type: pay } }))}>
              Create invoice
            </button>
          </>
        )}
        {canManage(user) && t.status !== "void" && (
          <button
            className="danger"
            disabled={busy}
            onClick={() => {
              const reason = prompt("Why is this ticket being voided?");
              if (reason) act(() => api(`/api/tickets/${t.id}/void`, { body: { reason } }));
            }}
          >
            Void
          </button>
        )}
      </div>
    </Modal>
  );
}
