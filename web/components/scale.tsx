"use client";
// Editor for a station's scale settings. This is what replaced the seven
// copies of the old Java screen: port, line settings and protocol are data.

export type ScaleConfig = {
  transport: { kind: "serial"; port: string; baud: number; framing: string } | { kind: "tcp"; host: string; port: number };
  profile_id?: string | null;
  profile?: any;
  stability?: { window_ms: number; tolerance_kg: number; min_samples: number };
  allow_manual?: boolean;
  max_capacity_kg?: number;
  min_weight_kg?: number;
};

export const PROFILES = [
  { value: "toledo", label: "Toledo continuous (Toledo, Keli, most Chinese indicators)" },
  { value: "st-gs", label: "ST,GS / US,GS (A&D, CAS style)" },
  { value: "yaohua-reversed", label: "Yaohua XK3190 reversed '=' (tF=0)" },
  { value: "weight-unit", label: "Weight + unit (Rinstrum / Avery / Cardinal)" },
  { value: "sign6", label: "Sign + 6 digits (+000140)" },
  { value: "generic-number", label: "Generic: first number in each frame" },
];

export const defaultScale = (): ScaleConfig => ({
  transport: { kind: "serial", port: "COM4", baud: 2400, framing: "8N1" },
  profile_id: "generic-number",
  stability: { window_ms: 1500, tolerance_kg: 20, min_samples: 3 },
  allow_manual: false,
  max_capacity_kg: 100000,
  min_weight_kg: 50,
});

export function ScaleEditor({ value, onChange, ports }: { value: ScaleConfig | null; onChange: (v: ScaleConfig) => void; ports?: string[] }) {
  const v = value ?? defaultScale();
  const t: any = v.transport;
  const st = v.stability ?? { window_ms: 1500, tolerance_kg: 20, min_samples: 3 };
  const setT = (p: any) => onChange({ ...v, transport: { ...t, ...p } });
  return (
    <div className="card" style={{ background: "var(--bg)" }}>
      <div className="grid g2">
        <div className="field">
          <label>Connection</label>
          <select
            value={t.kind}
            onChange={(e) =>
              onChange({
                ...v,
                transport: e.target.value === "tcp" ? { kind: "tcp", host: "192.168.1.50", port: 4001 } : { kind: "serial", port: "COM4", baud: 2400, framing: "8N1" },
              })
            }
          >
            <option value="serial">Serial cable (RS-232 / RS-485 / USB adapter)</option>
            <option value="tcp">Network (indicator with Ethernet / serial server)</option>
          </select>
        </div>
        {t.kind === "serial" ? (
          <>
            <div className="field">
              <label>Port</label>
              <input list="wb-ports" value={t.port} onChange={(e) => setT({ port: e.target.value })} placeholder="COM4 or /dev/ttyUSB0" />
              <datalist id="wb-ports">{ports?.map((p) => <option key={p} value={p} />)}</datalist>
            </div>
            <div className="field">
              <label>Baud rate</label>
              <select value={t.baud} onChange={(e) => setT({ baud: Number(e.target.value) })}>
                {[1200, 2400, 4800, 9600, 19200].map((b) => (
                  <option key={b} value={b}>{b}</option>
                ))}
              </select>
            </div>
            <div className="field">
              <label>Data bits / parity / stop bits</label>
              <select value={t.framing} onChange={(e) => setT({ framing: e.target.value })}>
                {["8N1", "7E1", "8E1", "7O1", "8N2"].map((f) => (
                  <option key={f} value={f}>{f}</option>
                ))}
              </select>
            </div>
          </>
        ) : (
          <>
            <div className="field">
              <label>Host / IP</label>
              <input value={t.host} onChange={(e) => setT({ host: e.target.value })} />
            </div>
            <div className="field">
              <label>TCP port</label>
              <input type="number" value={t.port} onChange={(e) => setT({ port: Number(e.target.value) })} />
            </div>
          </>
        )}
        <div className="field" style={{ gridColumn: "1/-1" }}>
          <label>Indicator protocol</label>
          <select value={v.profile ? "__custom" : v.profile_id ?? "generic-number"} onChange={(e) => onChange({ ...v, profile: undefined, profile_id: e.target.value })}>
            {v.profile && <option value="__custom">Custom / learned: {v.profile.name}</option>}
            {PROFILES.map((p) => (
              <option key={p.value} value={p.value}>{p.label}</option>
            ))}
          </select>
        </div>
        <div className="field">
          <label>Stable when steady for (ms)</label>
          <input type="number" value={st.window_ms} onChange={(e) => onChange({ ...v, stability: { ...st, window_ms: Number(e.target.value) } })} />
        </div>
        <div className="field">
          <label>...within (kg)</label>
          <input type="number" value={st.tolerance_kg} onChange={(e) => onChange({ ...v, stability: { ...st, tolerance_kg: Number(e.target.value) } })} />
        </div>
        <div className="field">
          <label>Empty deck below (kg)</label>
          <input type="number" value={v.min_weight_kg ?? 50} onChange={(e) => onChange({ ...v, min_weight_kg: Number(e.target.value) })} />
        </div>
        <div className="field">
          <label>Scale capacity (kg)</label>
          <input type="number" value={v.max_capacity_kg ?? 100000} onChange={(e) => onChange({ ...v, max_capacity_kg: Number(e.target.value) })} />
        </div>
        <div className="field">
          <label className="check">
            <input type="checkbox" checked={!!v.allow_manual} onChange={(e) => onChange({ ...v, allow_manual: e.target.checked })} />
            Allow supervisors to type a weight when the scale is down
          </label>
        </div>
      </div>
    </div>
  );
}
