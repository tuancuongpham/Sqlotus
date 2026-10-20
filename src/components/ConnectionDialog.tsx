import { useEffect, useState } from "react";
import { api, errorText, newId } from "../api";
import { toast, update } from "../notify";
import type { ConnectionConfig, Dialect, Environment, SavedConnection } from "../types";
import { DoctorDialog } from "./DoctorDialog";

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

const ENVIRONMENTS: { value: Environment; label: string; color: string }[] = [
  { value: "dev", label: "Development", color: "#27ae60" },
  { value: "test", label: "Test", color: "#2980b9" },
  { value: "staging", label: "Staging", color: "#d35400" },
  { value: "prod", label: "Production", color: "#c0392b" },
];
const ENV_ORDER = ["prod", "staging", "test", "dev", ""];

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

  const [doctorError, setDoctorError] = useState<string | null>(null);

  const save = async (announce = false) => {
    const c = normalized();
    await api.saveConnection(c, savePassword && password ? password : null, !savePassword);
    setCfg(c);
    await reload();
    if (announce) {
      const pw = c.auth === "windows" ? "" : savePassword ? (password ? " Password saved in Windows Credential Manager." : "") : " Password not saved.";
      toast.success(`Connection "${c.name}" saved`, pw.trim() || undefined);
    }
    return c;
  };

  const test = async () => {
    setBusy(true);
    setStatus(null);
    const c = normalized();
    const id = toast.progress(`Testing ${c.name}…`, c.host);
    try {
      const dbs = await api.testConnection(c, password || null);
      setStatus({ ok: true, text: `Connected. ${dbs.length} databases.` });
      update(id, "success", "Connection works", `${dbs.length} database(s) available`);
    } catch (e) {
      const err = errorText(e);
      setStatus({ ok: false, text: err });
      update(id, "error", "Connection failed", "The Connection Doctor shows why and how to fix it.", [
        { label: "Diagnose", run: () => setDoctorError(err) },
      ]);
      setDoctorError(err);
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
    if (!confirm(`Delete connection "${cfg.name}"?`)) {
      toast.info("Connection kept");
      return;
    }
    try {
      await api.deleteConnection(cfg.id);
      toast.success(`Connection "${cfg.name}" deleted`, "Its saved password was removed too.");
      setCfg(empty());
      await reload();
    } catch (e) {
      toast.error("Could not delete the connection", errorText(e));
    }
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
            {ENV_ORDER.map((env) => {
              const group = list.filter((c) => (c.environment ?? "") === env);
              if (group.length === 0) return null;
              const label = ENVIRONMENTS.find((e) => e.value === env)?.label;
              return (
                <div key={env || "none"}>
                  {label && list.some((c) => c.environment) && <div className={`conn-group env-${env}`}>{label}</div>}
                  {!label && list.some((c) => c.environment) && <div className="conn-group">Other</div>}
                  {group.map((c) => (
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
              );
            })}
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
              Environment
              <select
                value={cfg.environment ?? ""}
                onChange={(e) => {
                  const env = (e.target.value || null) as Environment | null;
                  const preset = ENVIRONMENTS.find((x) => x.value === env);
                  setCfg((c) => ({
                    ...c,
                    environment: env,
                    color: preset?.color ?? c.color,
                    // production: changes wait for an explicit Commit
                    manualCommit: env === "prod" ? true : c.manualCommit,
                  }));
                  if (env === "prod") toast.info("Production safety on", "Red tab colour, manual commit, and a confirmation before every change. Turn on Read-only too if you only read.");
                }}
              >
                <option value="">(none)</option>
                {ENVIRONMENTS.map((e) => (
                  <option key={e.value} value={e.value}>
                    {e.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="check" title="Block INSERT/UPDATE/DELETE/DDL on this connection — useful for production">
              <input type="checkbox" checked={!!cfg.readOnly} onChange={(e) => set("readOnly", e.target.checked)} />
              Read-only (block data and schema changes)
            </label>
            <label className="check" title="Start a transaction before changes; nothing is saved until you press Commit">
              <input type="checkbox" checked={!!cfg.manualCommit} onChange={(e) => set("manualCommit", e.target.checked)} />
              Manual commit (keep changes in a transaction until you commit)
            </label>
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
            {status && !status.ok && (
              <button className="primary" onClick={() => setDoctorError(status.text)}>
                🩺 Diagnose this error
              </button>
            )}
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
          <button onClick={() => setDoctorError("")} disabled={busy || !cfg.host.trim()} title="Check services, TCP/IP, ports, firewall and authentication of this server">
            🩺 Check server
          </button>
          <button
            onClick={() =>
              save(true).catch((e) => {
                setStatus({ ok: false, text: errorText(e) });
                toast.error("Could not save the connection", errorText(e));
              })
            }
            disabled={busy}
          >
            Save
          </button>
          <button className="primary" onClick={connect} disabled={busy}>
            Connect
          </button>
          <button onClick={onClose}>Close</button>
        </div>
      </div>
      {doctorError !== null && (
        <DoctorDialog
          config={normalized()}
          password={password || null}
          error={doctorError}
          onClose={() => setDoctorError(null)}
          onRetry={async (next) => {
            setDoctorError(null);
            setCfg(next);
            await reload();
            setBusy(true);
            const id = toast.progress(`Testing ${next.name}…`);
            try {
              const dbs = await api.testConnection(next, password || null);
              setStatus({ ok: true, text: `Connected. ${dbs.length} databases.` });
              update(id, "success", "Connection works now", "Press Connect to open it in the tab.");
            } catch (e) {
              setStatus({ ok: false, text: errorText(e) });
              update(id, "error", "Still failing", errorText(e).split(String.fromCharCode(10))[0], [{ label: "Diagnose", run: () => setDoctorError(errorText(e)) }]);
            } finally {
              setBusy(false);
            }
          }}
        />
      )}
    </div>
  );
}
