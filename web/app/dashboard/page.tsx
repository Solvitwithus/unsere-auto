"use client";
// Dashboard: totals for a date range, tonnage per day, top products/customers,
// eTIMS invoice health and whether each station is online.

import { useEffect, useState } from "react";
import { Shell, Status } from "@/components/ui";
import { api, daysAgo, dt, kg, money, today } from "@/lib/api";

export default function DashboardPage() {
  return <Shell>{() => <Dashboard />}</Shell>;
}

function Dashboard() {
  const [from, setFrom] = useState(daysAgo(29));
  const [to, setTo] = useState(today());
  const [d, setD] = useState<any>(null);
  const [err, setErr] = useState("");
  useEffect(() => {
    api(`/api/reports/summary?from=${from}&to=${to}`).then(setD).catch((e) => setErr(e.message));
  }, [from, to]);
  if (err) return <div className="alert">{err}</div>;
  if (!d) return <div className="muted">Loading...</div>;
  const t = d.totals ?? {};
  const s = d.sales ?? {};
  const max = Math.max(1, ...d.by_day.map((x: any) => x.net_kg));
  return (
    <>
      <div className="row" style={{ marginBottom: 14 }}>
        <h1 style={{ margin: 0 }}>Dashboard</h1>
        <div className="spacer" />
        <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} style={{ width: 160 }} />
        <span className="muted">to</span>
        <input type="date" value={to} onChange={(e) => setTo(e.target.value)} style={{ width: 160 }} />
      </div>
      <div className="grid g4" style={{ marginBottom: 14 }}>
        <Kpi v={(t.tickets ?? 0).toLocaleString()} l={`tickets (${t.open ?? 0} open, ${t.void ?? 0} void)`} />
        <Kpi v={`${((t.net_kg ?? 0) / 1000).toLocaleString("en-KE", { maximumFractionDigits: 1 })} t`} l="net weighed (completed)" />
        <Kpi v={money(s.total ?? 0)} l={`invoiced, VAT ${money(s.tax ?? 0)}`} />
        <Kpi v={`${s.signed ?? 0} / ${s.invoices ?? 0}`} l={`KRA-signed invoices${s.failed ? `, ${s.failed} failed` : ""}${s.pending ? `, ${s.pending} waiting` : ""}`} />
      </div>
      {t.manual_weights > 0 && <div className="notice">{t.manual_weights} ticket(s) in this period used a typed (manual) weight. Review them under Tickets.</div>}
      <div className="grid g2" style={{ marginBottom: 14 }}>
        <div className="card">
          <h2>Net tonnage per day</h2>
          <div className="bars" title="net tonnes per day">
            {d.by_day.map((x: any) => (
              <div key={x.day} style={{ height: `${(x.net_kg / max) * 100}%` }} title={`${x.day}: ${kg(x.net_kg)} in ${x.tickets} tickets`} />
            ))}
          </div>
          <div className="row small muted" style={{ marginTop: 6 }}>
            <span>{d.by_day[0]?.day}</span>
            <div className="spacer" />
            <span>{d.by_day[d.by_day.length - 1]?.day}</span>
          </div>
        </div>
        <div className="card">
          <h2>Stations</h2>
          <table>
            <tbody>
              {d.stations.map((st: any) => {
                const online = st.last_seen_at && Date.now() - new Date(st.last_seen_at).getTime() < 60000;
                const sc = st.last_status?.scale;
                return (
                  <tr key={st.code}>
                    <td>
                      <b>{st.code}</b> <span className="muted small">{st.site_name}</span>
                    </td>
                    <td>{online ? <span className="badge ok">online</span> : <span className="badge bad">offline</span>}</td>
                    <td className="small muted">
                      {sc?.receiving ? `scale OK (${kg(sc.reading?.weight_kg)})` : sc ? "no scale data" : ""}
                      {st.last_status?.pending ? `, ${st.last_status.pending} to sync` : ""}
                    </td>
                    <td className="small muted">{dt(st.last_seen_at)}</td>
                  </tr>
                );
              })}
              {!d.stations.length && (
                <tr>
                  <td className="muted">No stations yet. Add one under Sites, stations, users.</td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>
      <div className="grid g2">
        <Top title="Top products" rows={d.by_product} extra={(r) => <Status s={r.trans_type} />} />
        <Top title="Top customers / suppliers" rows={d.by_party} />
      </div>
    </>
  );
}

function Kpi({ v, l }: { v: string; l: string }) {
  return (
    <div className="card kpi">
      <div className="v">{v}</div>
      <div className="l">{l}</div>
    </div>
  );
}

function Top({ title, rows, extra }: { title: string; rows: any[]; extra?: (r: any) => React.ReactNode }) {
  return (
    <div className="card">
      <h2>{title}</h2>
      <table>
        <tbody>
          {rows.map((r, i) => (
            <tr key={i}>
              <td>{r.name}</td>
              {extra && <td>{extra(r)}</td>}
              <td className="right muted">{r.tickets}</td>
              <td className="right">{kg(r.net_kg)}</td>
            </tr>
          ))}
          {!rows.length && (
            <tr>
              <td className="muted">No completed tickets in this period.</td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}
