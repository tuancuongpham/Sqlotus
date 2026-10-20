import { useEffect, useState } from "react";
import { api, errorText, newId } from "../api";
import type { ConnectionConfig, Dialect, SavedConnection } from "../types";

const empty = (): ConnectionConfig => ({
  id: newId(),
  name: "",
  dialect: "mssql",
  host: "localhost",
  port: null,
  database: "",
  auth: "windows",
  user: "",
  encrypt: false,
  trustServerCertificate: true,
  color: null,
});

const COLORS = ["", "#c0392b", "#d35400", "#27ae60", "#2980b9", "#8e44ad"];

interface Props {
  onClose: () => void;
  /** Connect the current tab with this connection. */
  onConnect: (conn: SavedConnection, password: string | null) => void;
}

export function ConnectionDialog({ onClose, onConnect }: Props) {
  const [list, setList] = useState<SavedConnection[]>([]);
  const [cfg, setCfg] = useState<ConnectionConfig>(empty);
  const [password, setPassword] = useState("");
  const [savePassword, setSavePassword] = useState(true);
  const [status, setStatus] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = async () => setList(await api.listConnections());
  useEffect(() => {
    reload().then(() => undefined);
  }, []);

  const set = <K extends keyof ConnectionConfig>(k: K, v: ConnectionConfig[K]) => setCfg((c) => ({ ...c, [k]: v }));

  const select = (c: SavedConnection) => {
    setCfg({ ...c });
    setPassword("");
    setStatus(null);
  };

  const normalized = (): ConnectionConfig => ({
    ...cfg,
    name: cfg.name.trim() || `${cfg.host}${cfg.database ? ` / ${cfg.database}` : ""}`,
    port: cfg.port ? Number(cfg.port) : null,
    auth: cfg.dialect === "mssql" ? cfg.auth : "sql",
  });

  const save = async () => {
    const c = normalized();
    await api.saveConnection(c, savePassword && password ? password : null, !savePassword);
    setCfg(c);
    await reload();
    return c;
  };

  const test = async () => {
    setBusy(true);
    setStatus(null);
    try {
      const dbs = await api.testConnection(normalized(), password || null);
      setStatus({ ok: true, text: `Connected. ${dbs.length} databases.` });
    } catch (e) {
      setStatus({ ok: false, text: errorText(e) });
    } finally {
      setBusy(false);
    }
  };

  const connect = async () => {
    setBusy(true);
    try {
      const c = await save();
      const saved = (await api.listConnections()).find((x) => x.id === c.id);
      if (saved) onConnect(saved, password || null);
    } catch (e) {
      setStatus({ ok: false, text: errorText(e) });
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!list.some((x) => x.id === cfg.id)) return;
    if (!confirm(`Delete connection "${cfg.name}"?`)) return;
    await api.deleteConnection(cfg.id);
    setCfg(empty());
    await reload();
  };

  const isSaved = list.some((x) => x.id === cfg.id);
  const hasStoredPassword = list.find((x) => x.id === cfg.id)?.hasPassword ?? false;

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal conn-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">Connect to server</div>
        <div className="conn-body">
          <div className="conn-list">
            <button className="conn-new" onClick={() => select({ ...empty(), hasPassword: false })}>
              + New connection
            </button>
            {list.map((c) => (
              <div
                key={c.id}
                className={"conn-item" + (c.id === cfg.id ? " active" : "")}
                onClick={() => select(c)}
                onDoubleClick={() => onConnect(c, null)}
              >
                <span className="dot" style={{ background: c.color || "transparent" }} />
                <div>
                  <div>{c.name}</div>
                  <small>
                    {c.dialect} · {c.host}
                  </small>
                </div>
              </div>
            ))}
          </div>
          <div className="conn-form">
            <label>
              Name
              <input value={cfg.name} placeholder="(host / database)" onChange={(e) => set("name", e.target.value)} />
            </label>
            <label>
              Server type
              <select value={cfg.dialect} onChange={(e) => set("dialect", e.target.value as Dialect)}>
                <option value="mssql">SQL Server</option>
                <option value="postgres">PostgreSQL</option>
                <option value="mysql">MySQL / MariaDB</option>
              </select>
            </label>
            <div className="row2">
              <label>
                Server
                <input
                  value={cfg.host}
                  placeholder={cfg.dialect === "mssql" ? "host\\instance or host,port" : "host"}
                  onChange={(e) => set("host", e.target.value)}
                />
              </label>
              <label className="port">
                Port
                <input value={cfg.port ?? ""} placeholder="default" onChange={(e) => set("port", e.target.value ? Number(e.target.value) : null)} />
              </label>
            </div>
            {cfg.dialect === "mssql" && (
              <label>
                Authentication
                <select value={cfg.auth} onChange={(e) => set("auth", e.target.value as "sql" | "windows")}>
                  <option value="windows">Windows Authentication</option>
                  <option value="sql">SQL Server Authentication</option>
                </select>
              </label>
            )}
            {(cfg.dialect !== "mssql" || cfg.auth === "sql") && (
              <div className="row2">
                <label>
                  User
                  <input value={cfg.user ?? ""} onChange={(e) => set("user", e.target.value)} />
                </label>
                <label>
                  Password
                  <input
                    type="password"
                    value={password}
                    placeholder={hasStoredPassword ? "(saved)" : ""}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                </label>
              </div>
            )}
            {(cfg.dialect !== "mssql" || cfg.auth === "sql") && (
              <label className="check">
                <input type="checkbox" checked={savePassword} onChange={(e) => setSavePassword(e.target.checked)} />
                Remember password (Windows Credential Manager)
              </label>
            )}
            <label>
              Database
              <input value={cfg.database ?? ""} placeholder="(default)" onChange={(e) => set("database", e.target.value)} />
            </label>
            <div className="row2">
              <label className="check">
                <input type="checkbox" checked={cfg.encrypt} onChange={(e) => set("encrypt", e.target.checked)} />
                Encrypt connection
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={cfg.trustServerCertificate}
                  onChange={(e) => set("trustServerCertificate", e.target.checked)}
                />
                Trust server certificate
              </label>
            </div>
            <label>
              Tab colour
              <div className="colors">
                {COLORS.map((c) => (
                  <span
                    key={c}
                    className={"swatch" + ((cfg.color ?? "") === c ? " active" : "")}
                    style={{ background: c || "transparent" }}
                    title={c ? c : "none"}
                    onClick={() => set("color", c || null)}
                  />
                ))}
              </div>
            </label>
            {status && <div className={status.ok ? "status-ok" : "status-error"}>{status.text}</div>}
          </div>
        </div>
        <div className="modal-actions">
          {isSaved && (
            <button className="danger" onClick={remove}>
              Delete
            </button>
          )}
          <span className="spacer" />
          <button onClick={test} disabled={busy}>
            Test
          </button>
          <button onClick={() => save().catch((e) => setStatus({ ok: false, text: errorText(e) }))} disabled={busy}>
            Save
          </button>
          <button className="primary" onClick={connect} disabled={busy}>
            Connect
          </button>
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
