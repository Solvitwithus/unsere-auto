"use client";
// Sales (invoices) in the eTIMS format. Each sale shows its KRA amounts per
// line (prc, splyAmt, taxblAmt, taxAmt, totAmt) and its eTIMS status:
//   DISABLED  - eTIMS not configured / proforma / cancelled
//   PENDING   - waiting to be sent
//   SUBMITTED - eTIMS accepted it, waiting for the KRA signature
//   SIGNED    - KRA signed it (CU invoice no., signature, QR link stored)
//   FAILED    - eTIMS refused it or was unreachable; see the error, fix, Retry

import { useEffect, useState } from "react";
import { Modal, Shell, Status } from "@/components/ui";
import { api, apiText, canManage, daysAgo, dt, money, printText, today, type User } from "@/lib/api";

export default function SalesPage() {
  return <Shell>{(u) => <Sales user={u} />}</Shell>;
}

function Sales({ user }: { user: User }) {
  const [f, setF] = useState({ from: daysAgo(29), to: today(), etims_status: "", q: "" });
  const [rows, setRows] = useState<any[]>([]);
  const [err, setErr] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const load = () =>
    api<any[]>(`/api/sales?${new URLSearchParams(Object.entries(f).filter(([, v]) => v) as [string, string][])}`).then(setRows).catch((e) => setErr(e.message));
  useEffect(() => {
    const t = setTimeout(load, 250);
    return () => clearTimeout(t);
  }, [f]);
  useEffect(() => {
    const id = new URLSearchParams(window.location.search).get("open");
    if (id) setOpen(id);
  }, []);
  const set = (k: string, v: string) => setF((o) => ({ ...o, [k]: v }));
  const total = rows.filter((r) => r.status === "APPROVED").reduce((s, r) => s + (r.total_amount ?? 0), 0);

  return (
    <>
      <div className="row" style={{ marginBottom: 12 }}>
        <h1 style={{ margin: 0 }}>Sales (eTIMS)</h1>
        <span className="muted">{rows.length} invoices, {money(total)}</span>
        <div className="spacer" />
        <button className="primary" onClick={() => setCreating(true)}>+ New sale</button>
      </div>
      <div className="row card" style={{ marginBottom: 12 }}>
        <input type="date" value={f.from} onChange={(e) => set("from", e.target.value)} style={{ width: 150 }} />
        <input type="date" value={f.to} onChange={(e) => set("to", e.target.value)} style={{ width: 150 }} />
        <select value={f.etims_status} onChange={(e) => set("etims_status", e.target.value)} style={{ width: 170 }}>
          <option value="">Any eTIMS status</option>
          {["PENDING", "SUBMITTED", "SIGNED", "FAILED", "DISABLED"].map((s) => <option key={s}>{s}</option>)}
        </select>
        <input placeholder="Invoice no, buyer, plate" value={f.q} onChange={(e) => set("q", e.target.value)} style={{ flex: 1, minWidth: 200 }} />
      </div>
      {err && <div className="alert">{err}</div>}
      <div className="card tablewrap" style={{ padding: 0 }}>
        <table>
          <thead>
            <tr><th>Invoice</th><th>Date</th><th>Buyer</th><th>PIN</th><th>Ticket</th><th>Payment</th><th className="right">VAT</th><th className="right">Total</th><th>Status</th><th>eTIMS</th></tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.id} className="click" onClick={() => setOpen(r.id)}>
                <td className="mono">{r.sale_no}</td>
                <td>{dt(r.sale_date)}</td>
                <td>{r.buyer_name}</td>
                <td className="mono small">{r.buyer_tin}</td>
                <td className="mono small">{r.ticket_no} {r.vehicle_plate}</td>
                <td>{r.payment_type}</td>
                <td className="right">{money(r.tax_amount)}</td>
                <td className="right"><b>{money(r.total_amount)}</b></td>
                <td><Status s={r.status} /></td>
                <td title={r.etims_error ?? ""}><Status s={r.etims_status} /> {r.etims_invoice_code && <span className="small mono">{r.etims_invoice_code}</span>}</td>
              </tr>
            ))}
            {!rows.length && <tr><td colSpan={10} className="muted">No sales in this period.</td></tr>}
          </tbody>
        </table>
      </div>
      {open && <SaleModal id={open} user={user} onClose={() => { setOpen(null); load(); }} />}
      {creating && <NewSale onClose={() => setCreating(false)} onCreated={(id) => { setCreating(false); load(); setOpen(id); }} />}
    </>
  );
}

function SaleModal({ id, user, onClose }: { id: string; user: User; onClose: () => void }) {
  const [d, setD] = useState<any>(null);
  const [err, setErr] = useState("");
  const [body, setBody] = useState<any>(null);
  const load = () => api(`/api/sales/${id}`).then(setD).catch((e) => setErr(e.message));
  useEffect(() => { load(); }, [id]);
  if (!d) return <Modal title="Sale" onClose={onClose}>{err ? <div className="alert">{err}</div> : "Loading..."}</Modal>;
  const s = d.sale;
  const scu = s.etimsScu ?? {};
  const act = (fn: () => Promise<any>) => fn().then(load).catch((e) => setErr(e.message));
  return (
    <Modal title={`Invoice ${s.saleNo}`} onClose={onClose} wide>
      {err && <div className="alert">{err}</div>}
      <div className="grid g2" style={{ marginBottom: 12 }}>
        <div>
          <div><span className="muted">Buyer:</span> <b>{s.buyerName}</b> {s.buyerTin && <span className="mono">({s.buyerTin})</span>}</div>
          {(s.buyerPhone || s.buyerEmail) && <div className="muted small">{s.buyerPhone} {s.buyerEmail}</div>}
          <div><span className="muted">Date:</span> {dt(s.saleDate)} · <span className="muted">Payment:</span> {s.paymentType} · <span className="muted">Type:</span> {s.saleType}</div>
          <div><span className="muted">Prices:</span> {s.pricesIncludeTax ? "VAT inclusive" : "VAT exclusive"} · <span className="muted">By:</span> {s.actorName}</div>
          {s.remark && <div className="small muted">{s.remark}</div>}
        </div>
        <div className="card" style={{ background: "var(--bg)" }}>
          <div className="row"><b>KRA eTIMS</b><Status s={s.etimsStatus} /><Status s={s.status} /></div>
          {!d.etims_configured && <div className="small muted">eTIMS is not configured on this server, so sales are recorded only.</div>}
          {s.etimsInvoiceCode && <div className="small">eTIMS invoice: <span className="mono">{s.etimsInvoiceCode}</span></div>}
          {scu.cuInvoiceNo && <div className="small">CU invoice no: <span className="mono">{scu.cuInvoiceNo}</span></div>}
          {scu.internalData && <div className="small">Internal data: <span className="mono">{scu.internalData}</span></div>}
          {scu.receiptSignature && <div className="small">Signature: <span className="mono">{scu.receiptSignature}</span></div>}
          {scu.qrUrl && <div className="small"><a href={scu.qrUrl} target="_blank" rel="noreferrer">Verify on KRA</a></div>}
          {s.etimsError && <div className="small" style={{ color: "var(--bad)" }}>{s.etimsError}</div>}
          {s.etimsAttempts > 0 && <div className="small muted">Attempts: {s.etimsAttempts}</div>}
        </div>
      </div>
      <div className="tablewrap">
        <table>
          <thead>
            <tr><th>#</th><th>Item</th><th>Code / class.</th><th className="right">Qty</th><th className="right">Unit price</th><th>Tax</th>
              <th className="right">prc</th><th className="right">splyAmt</th><th className="right">dcAmt</th><th className="right">taxblAmt</th><th className="right">taxAmt</th><th className="right">totAmt</th></tr>
          </thead>
          <tbody>
            {d.items.map((i: any) => (
              <tr key={i.seq}>
                <td>{i.seq}</td><td>{i.name}</td><td className="mono small">{i.itemCode ?? i.classification}</td>
                <td className="right">{i.quantity} {i.quantityUnit}</td><td className="right">{money(i.unitPrice)}</td>
                <td>{i.taxTypeCode} ({i.vatRate}%)</td><td className="right">{money(i.prc)}</td><td className="right">{money(i.splyAmt)}</td>
                <td className="right">{money(i.dcAmt)}</td><td className="right">{money(i.taxblAmt)}</td><td className="right">{money(i.taxAmt)}</td><td className="right"><b>{money(i.totAmt)}</b></td>
              </tr>
            ))}
            <tr><td colSpan={9} className="right muted">Taxable / VAT / Total</td><td className="right">{money(s.taxableAmount)}</td><td className="right">{money(s.taxAmount)}</td><td className="right"><b>{money(s.totalAmount)}</b></td></tr>
          </tbody>
        </table>
      </div>
      <div className="row" style={{ marginTop: 14 }}>
        <button onClick={() => apiText(`/api/sales/${id}/receipt.txt`).then((t) => printText(t, s.saleNo))}>Print receipt</button>
        <button onClick={() => api(`/api/sales/${id}/etims-request`).then(setBody)}>Show eTIMS request</button>
        <div className="spacer" />
        {canManage(user) && ["FAILED", "PENDING", "SUBMITTED", "DISABLED"].includes(s.etimsStatus) && s.status === "APPROVED" && s.saleType === "NORMAL" && d.etims_configured && (
          <button className="primary" onClick={() => act(() => api(`/api/sales/${id}/retry`, { method: "POST" }))}>Send to eTIMS now</button>
        )}
        {canManage(user) && s.status === "APPROVED" && !["SIGNED", "SUBMITTED"].includes(s.etimsStatus) && (
          <button className="danger" onClick={() => { const r = prompt("Reason for cancelling?"); if (r) act(() => api(`/api/sales/${id}/cancel`, { body: { reason: r } })); }}>Cancel sale</button>
        )}
      </div>
      {body && <pre className="ticket" style={{ marginTop: 12 }}>{JSON.stringify(body, null, 2)}</pre>}
    </Modal>
  );
}

type Line = { product_id: string; name: string; quantity: string; unit_price: string };

/** A sale typed in by hand (not from a ticket), e.g. selling a bag of cement. */
function NewSale({ onClose, onCreated }: { onClose: () => void; onCreated: (id: string) => void }) {
  const [parties, setParties] = useState<any[]>([]);
  const [products, setProducts] = useState<any[]>([]);
  const [partyId, setPartyId] = useState("");
  const [buyer, setBuyer] = useState({ name: "", tin: "", phone: "" });
  const [pay, setPay] = useState("Cash");
  const [remark, setRemark] = useState("");
  const [lines, setLines] = useState<Line[]>([{ product_id: "", name: "", quantity: "1", unit_price: "" }]);
  const [err, setErr] = useState("");
  useEffect(() => {
    api<any[]>("/api/parties").then(setParties);
    api<any[]>("/api/products").then(setProducts);
  }, []);
  const setLine = (i: number, p: Partial<Line>) => setLines((ls) => ls.map((l, j) => (j === i ? { ...l, ...p } : l)));
  async function save() {
    setErr("");
    try {
      const r = await api("/api/sales", {
        body: {
          party_id: partyId || null,
          buyer: partyId ? null : { name: buyer.name, tin: buyer.tin || null, phone: buyer.phone || null },
          payment_type: pay,
          remark: remark || null,
          items: lines.filter((l) => l.product_id || l.name).map((l) => ({
            productId: l.product_id || null,
            name: l.name || null,
            quantity: Number(l.quantity),
            unitPrice: l.unit_price === "" ? null : Number(l.unit_price),
          })),
        },
      });
      onCreated(r.sale.id);
    } catch (e: any) {
      setErr(e.message);
    }
  }
  return (
    <Modal title="New sale" onClose={onClose} wide>
      {err && <div className="alert">{err}</div>}
      <div className="grid g2">
        <div className="field">
          <label>Customer</label>
          <select value={partyId} onChange={(e) => setPartyId(e.target.value)}>
            <option value="">Walk-in / type below</option>
            {parties.filter((p) => p.kind !== "supplier").map((p) => <option key={p.id} value={p.id}>{p.name}{p.tin ? ` (${p.tin})` : ""}</option>)}
          </select>
        </div>
        <div className="field">
          <label>Payment</label>
          <select value={pay} onChange={(e) => setPay(e.target.value)}>
            {["Cash", "Credit", "Mobile Money", "Bank Transfer", "Cheque", "Card"].map((p) => <option key={p}>{p}</option>)}
          </select>
        </div>
        {!partyId && (
          <>
            <div className="field"><label>Buyer name</label><input value={buyer.name} onChange={(e) => setBuyer({ ...buyer, name: e.target.value })} placeholder="Walk-in Customer" /></div>
            <div className="field"><label>Buyer KRA PIN (optional)</label><input value={buyer.tin} onChange={(e) => setBuyer({ ...buyer, tin: e.target.value.toUpperCase() })} placeholder="P051234567X" /></div>
          </>
        )}
      </div>
      <table style={{ marginBottom: 10 }}>
        <thead><tr><th>Product</th><th>Quantity</th><th>Unit price (blank = list price)</th><th /></tr></thead>
        <tbody>
          {lines.map((l, i) => {
            const p = products.find((x) => x.id === l.product_id);
            return (
              <tr key={i}>
                <td>
                  <select value={l.product_id} onChange={(e) => setLine(i, { product_id: e.target.value })}>
                    <option value="">Choose...</option>
                    {products.map((p) => <option key={p.id} value={p.id}>{p.name} ({money(p.unit_price)}/{p.quantity_unit}, tax {p.tax_type_code})</option>)}
                  </select>
                </td>
                <td><input type="number" step="0.01" value={l.quantity} onChange={(e) => setLine(i, { quantity: e.target.value })} style={{ width: 110 }} /> {p?.quantity_unit}</td>
                <td><input type="number" step="0.01" value={l.unit_price} placeholder={p ? String(p.unit_price) : ""} onChange={(e) => setLine(i, { unit_price: e.target.value })} style={{ width: 140 }} /></td>
                <td>{lines.length > 1 && <button onClick={() => setLines(lines.filter((_, j) => j !== i))}>✕</button>}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <div className="row">
        <button onClick={() => setLines([...lines, { product_id: "", name: "", quantity: "1", unit_price: "" }])}>+ Line</button>
        <input placeholder="Remark (optional)" value={remark} onChange={(e) => setRemark(e.target.value)} style={{ flex: 1 }} />
        <button className="primary" onClick={save}>Create invoice</button>
      </div>
      <p className="small muted">Amounts and VAT are calculated by the server with the same rules as eTIMS. A sale with eTIMS configured is sent to KRA automatically.</p>
    </Modal>
  );
}
