"use client";
// Generic list + add/edit form, driven by a field list. Used for customers,
// products, vehicles, sites, users and stations so each screen is a few lines.

import { useEffect, useState, type ReactNode } from "react";
import { api } from "@/lib/api";
import { Modal } from "./ui";

export type Field = {
  key: string;
  label: string;
  type?: "text" | "number" | "select" | "bool" | "password" | "custom";
  options?: { value: string; label: string }[];
  /** hide from the list table */
  formOnly?: boolean;
  /** hide from the form */
  tableOnly?: boolean;
  help?: string;
  render?: (row: any) => ReactNode;
  /** for type "custom": draw your own editor */
  editor?: (value: any, set: (v: any) => void, row: any) => ReactNode;
  default?: any;
};

export function Crud({
  title, endpoint, fields, canEdit, rowKey = "id", afterSave, extraActions, query = "?all=true",
}: {
  title: string;
  endpoint: string;
  fields: Field[];
  canEdit: boolean;
  rowKey?: string;
  afterSave?: (result: any, isNew: boolean) => void;
  extraActions?: (row: any, reload: () => void) => ReactNode;
  query?: string;
}) {
  const [rows, setRows] = useState<any[]>([]);
  const [err, setErr] = useState("");
  const [edit, setEdit] = useState<any | null>(null);
  const [q, setQ] = useState("");

  const load = () => api<any[]>(endpoint + query).then(setRows).catch((e) => setErr(e.message));
  useEffect(() => {
    load();
  }, [endpoint]);

  const shown = rows.filter((r) => !q || JSON.stringify(r).toLowerCase().includes(q.toLowerCase()));
  const blank = () => Object.fromEntries(fields.map((f) => [f.key, f.default ?? (f.type === "bool" ? true : f.type === "number" ? null : "")]));

  return (
    <div>
      <div className="row" style={{ marginBottom: 10 }}>
        <h2 style={{ margin: 0 }}>{title}</h2>
        <span className="muted small">{rows.length}</span>
        <div className="spacer" />
        <input placeholder="Search" value={q} onChange={(e) => setQ(e.target.value)} style={{ width: 200 }} />
        {canEdit && (
          <button className="primary" onClick={() => setEdit(blank())}>
            + Add
          </button>
        )}
      </div>
      {err && <div className="alert">{err}</div>}
      <div className="tablewrap card" style={{ padding: 0 }}>
        <table>
          <thead>
            <tr>
              {fields.filter((f) => !f.formOnly).map((f) => (
                <th key={f.key}>{f.label}</th>
              ))}
              {extraActions && <th />}
            </tr>
          </thead>
          <tbody>
            {shown.map((r) => (
              <tr key={r[rowKey]} className={canEdit ? "click" : ""} onClick={() => canEdit && setEdit({ ...r })}>
                {fields.filter((f) => !f.formOnly).map((f) => (
                  <td key={f.key}>{f.render ? f.render(r) : fmt(r[f.key], f)}</td>
                ))}
                {extraActions && <td onClick={(e) => e.stopPropagation()}>{extraActions(r, load)}</td>}
              </tr>
            ))}
            {!shown.length && (
              <tr>
                <td colSpan={fields.length + 1} className="muted">
                  Nothing here yet.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {edit && (
        <EditForm
          title={edit[rowKey] ? `Edit ${title.toLowerCase()}` : `New ${title.toLowerCase()}`}
          fields={fields.filter((f) => !f.tableOnly)}
          value={edit}
          onClose={() => setEdit(null)}
          onSave={async (v) => {
            const isNew = !v[rowKey];
            const res = await api(isNew ? endpoint : `${endpoint}/${v[rowKey]}`, { method: isNew ? "POST" : "PUT", body: clean(v, fields) });
            setEdit(null);
            load();
            afterSave?.(res, isNew);
          }}
        />
      )}
    </div>
  );
}

function fmt(v: any, f: Field) {
  if (f.type === "bool") return v ? "✓" : "–";
  if (f.type === "select") return f.options?.find((o) => o.value === v)?.label ?? v ?? "";
  if (f.type === "password") return "";
  if (v === null || v === undefined) return "";
  if (typeof v === "object") return JSON.stringify(v).slice(0, 40);
  return String(v);
}

/** Empty strings become null for optional ids/numbers; numbers are parsed. */
function clean(v: any, fields: Field[]) {
  const out: any = { ...v };
  for (const f of fields) {
    if (f.type === "number") out[f.key] = v[f.key] === "" || v[f.key] === null ? null : Number(v[f.key]);
    if (f.type === "select" && v[f.key] === "") out[f.key] = null;
    if (f.type === "password" && !v[f.key]) delete out[f.key];
  }
  return out;
}

function EditForm({ title, fields, value, onClose, onSave }: { title: string; fields: Field[]; value: any; onClose: () => void; onSave: (v: any) => Promise<void> }) {
  const [v, setV] = useState(value);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const set = (k: string, x: any) => setV((o: any) => ({ ...o, [k]: x }));
  return (
    <Modal title={title} onClose={onClose}>
      {err && <div className="alert">{err}</div>}
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          setErr("");
          try {
            await onSave(v);
          } catch (e: any) {
            setErr(e.message);
          } finally {
            setBusy(false);
          }
        }}
      >
        <div className="grid g2">
          {fields.map((f) => (
            <div className="field" key={f.key} style={f.type === "custom" ? { gridColumn: "1/-1" } : undefined}>
              {f.type === "bool" ? (
                <label className="check">
                  <input type="checkbox" checked={!!v[f.key]} onChange={(e) => set(f.key, e.target.checked)} /> {f.label}
                </label>
              ) : (
                <>
                  <label>{f.label}</label>
                  {f.type === "select" ? (
                    <select value={v[f.key] ?? ""} onChange={(e) => set(f.key, e.target.value)}>
                      {f.options?.map((o) => (
                        <option key={o.value} value={o.value}>
                          {o.label}
                        </option>
                      ))}
                    </select>
                  ) : f.type === "custom" && f.editor ? (
                    f.editor(v[f.key], (x) => set(f.key, x), v)
                  ) : (
                    <input
                      type={f.type === "number" ? "number" : f.type === "password" ? "password" : "text"}
                      step="any"
                      value={v[f.key] ?? ""}
                      placeholder={f.type === "password" && v.id ? "(unchanged)" : ""}
                      onChange={(e) => set(f.key, e.target.value)}
                    />
                  )}
                </>
              )}
              {f.help && <div className="small muted">{f.help}</div>}
            </div>
          ))}
        </div>
        <div className="row">
          <div className="spacer" />
          <button type="button" onClick={onClose}>
            Cancel
          </button>
          <button className="primary" disabled={busy}>
            {busy ? "Saving..." : "Save"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
