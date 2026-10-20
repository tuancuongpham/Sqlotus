import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, errorText } from "../api";
import { toast } from "../notify";
import type { Dialect, SessionInfo } from "../types";

function duration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

/** Sessions on the server, blocking chains, and cancel / kill. */
export function ActivityDialog({
  connectionId,
  database,
  dialect,
  server,
  onOpenScript,
  onClose,
}: {
  connectionId: string;
  database: string;
  dialect: Dialect;
  server: string;
  onOpenScript: (title: string, text: string) => void;
  onClose: () => void;
}) {
  const [sessions, setSessions] = useState<SessionInfo[] | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [auto, setAuto] = useState(true);
  const [onlyActive, setOnlyActive] = useState(false);
  const [selected, setSelected] = useState<number | null>(null);
  const [updated, setUpdated] = useState<Date | null>(null);
  const [asking, setAsking] = useState<{ s: SessionInfo; whole: boolean; text: string } | null>(null);
  const busy = useRef(false);

  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      const a = await api.serverActivity(connectionId, database, dialect);
      setSessions(a.sessions);
      setNote(a.note);
      setError("");
      setUpdated(new Date());
    } catch (e) {
      setError(errorText(e));
    } finally {
      busy.current = false;
    }
  }, [connectionId, database, dialect]);

  useEffect(() => {
    void refresh();
    if (!auto) return;
    const t = window.setInterval(() => void refresh(), 3000);
    return () => window.clearInterval(t);
  }, [refresh, auto]);

  // close the monitor connection with the window
  useEffect(() => () => void api.closeActivity(connectionId, database).catch(() => undefined), [connectionId, database]);

  const visible = useMemo(
    () => (sessions ?? []).filter((s) => !onlyActive || s.running || s.blocking > 0 || s.blockedBy !== 0 || s.openTransaction),
    [sessions, onlyActive],
  );
  const heads = (sessions ?? []).filter((s) => s.blocking > 0 && s.blockedBy === 0);
  const current = (sessions ?? []).find((s) => s.id === selected) ?? null;

  const ask = (s: SessionInfo, whole: boolean) => {
    const what = whole || dialect === "mssql" ? `End session ${s.id} (${s.login}@${s.host})` : `Cancel the running query of session ${s.id}`;
    const tx = s.openTransaction && (whole || dialect === "mssql") ? " Its open transaction will be rolled back." : "";
    setAsking({ s, whole, text: `${what}?${tx}` });
  };

  const kill = async (s: SessionInfo, whole: boolean) => {
    setAsking(null);
    try {
      const msg = await api.killSession(connectionId, database, dialect, s.id, whole);
      toast.success(msg);
      void refresh();
    } catch (e) {
      toast.error("Could not stop the session", errorText(e));
    }
  };

  const chain = (head: SessionInfo, depth = 0): React.ReactNode[] => {
    const waiters = (sessions ?? []).filter((s) => s.blockedBy === head.id && s.id !== head.id);
    return [
      <div key={`${head.id}-${depth}`} className="blocking-row" style={{ paddingLeft: depth * 18 }} onClick={() => setSelected(head.id)}>
        {depth === 0 ? "🔒" : "↳"} <b>{head.id}</b> {head.login}@{head.host} · {head.database} ·{" "}
        {depth === 0 ? `blocks ${head.blocking} session(s)` : `waiting ${duration(head.elapsedMs)} (${head.wait || head.status})`}
        {head.openTransaction && depth === 0 && " · open transaction"}
        <span className="blocking-sql" title={head.running ? "Running now" : "Last statement: the locks may come from an earlier statement of the transaction"}>
          {head.running ? "" : "last: "}
          {head.sql.replace(/\s+/g, " ").slice(0, 120)}
        </span>
      </div>,
      ...(depth < 8 ? waiters.flatMap((w) => chain(w, depth + 1)) : []),
    ];
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal activity-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">Activity — {server}</div>
        <div className="activity-controls">
          <label className="check">
            <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} />
            Refresh every 3 s
          </label>
          <label className="check">
            <input type="checkbox" checked={onlyActive} onChange={(e) => setOnlyActive(e.target.checked)} />
            Only running, blocking or in a transaction
          </label>
          <button onClick={() => void refresh()}>Refresh</button>
          <span className="spacer" />
          <small>
            {sessions ? `${sessions.length} session(s)` : ""}
            {updated && ` · updated ${updated.toLocaleTimeString()}`}
          </small>
        </div>
        {error && <div className="status-error activity-error">{error}</div>}
        {note && <div className="settings-hint activity-note">ℹ {note}</div>}
        {heads.length > 0 && (
          <div className="blocking-chains">
            <b>Blocking</b>
            {heads.flatMap((h) => chain(h))}
          </div>
        )}
        <div className="activity-body">
          <div className="activity-table-wrap">
            {!sessions && !error ? (
              <div className="panel-empty">
                <span className="spinner" /> Reading sessions…
              </div>
            ) : (
              <table className="activity-table">
                <thead>
                  <tr>
                    <th>Id</th>
                    <th>Blocked by</th>
                    <th>Status</th>
                    <th>Time</th>
                    <th>Wait</th>
                    <th>Database</th>
                    <th>Login @ host</th>
                    <th>Program</th>
                    <th>Tran</th>
                    <th>SQL</th>
                  </tr>
                </thead>
                <tbody>
                  {visible.map((s) => (
                    <tr
                      key={s.id}
                      className={(s.id === selected ? "selected " : "") + (s.blocking > 0 ? "is-blocking " : "") + (s.blockedBy ? "is-blocked " : "") + (s.isSelf ? "is-self" : "")}
                      onClick={() => setSelected(s.id)}
                    >
                      <td>{s.id}</td>
                      <td>{s.blockedBy || ""}</td>
                      <td>{s.status}</td>
                      <td>{s.running || s.blockedBy ? duration(s.elapsedMs) : ""}</td>
                      <td>{s.wait}</td>
                      <td>{s.database}</td>
                      <td>
                        {s.login}@{s.host}
                      </td>
                      <td>{s.isSelf ? "Sqlotus monitor" : s.program}</td>
                      <td>{s.openTransaction ? "●" : ""}</td>
                      <td className="activity-sql" title={s.sql}>
                        {s.sql.replace(/\s+/g, " ").slice(0, 200)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
          {current && (
            <div className="activity-detail">
              <div className="preview-bar">
                <b>Session {current.id}</b>
                <span className="spacer" />
                <button onClick={() => void navigator.clipboard.writeText(current.sql).then(() => toast.success("SQL copied"))} disabled={!current.sql}>
                  Copy SQL
                </button>
                <button onClick={() => onOpenScript(`Session ${current.id}`, current.sql)} disabled={!current.sql}>
                  Open in tab
                </button>
                {dialect !== "mssql" && (
                  <button onClick={() => ask(current, false)} disabled={current.isSelf || !current.running}>
                    Cancel query
                  </button>
                )}
                <button className="danger" onClick={() => ask(current, true)} disabled={current.isSelf}>
                  Kill session
                </button>
              </div>
              {!current.running && current.sql && (
                <small>Idle — this is its last statement. Locks held by an open transaction may come from earlier statements.</small>
              )}
              <pre>{current.sql || "(no SQL)"}</pre>
              <small>
                {current.command && `${current.command} · `}
                {current.cpuMs != null && `CPU ${duration(current.cpuMs)} · `}
                {current.reads != null && `${current.reads.toLocaleString()} reads · `}
                {current.blocking > 0 && `blocks ${current.blocking} · `}
                {current.openTransaction ? "open transaction" : "no open transaction"}
              </small>
            </div>
          )}
        </div>
        {asking && (
          <div className="activity-confirm">
            <span>{asking.text}</span>
            <button className="danger" onClick={() => void kill(asking.s, asking.whole)}>
              {asking.whole || dialect === "mssql" ? "Kill session" : "Cancel query"}
            </button>
            <button
              onClick={() => {
                setAsking(null);
                toast.info("Nothing was changed");
              }}
            >
              Keep it
            </button>
          </div>
        )}
        <div className="modal-actions">
          <small>Other sessions are visible with VIEW SERVER STATE (SQL Server) or pg_read_all_stats (PostgreSQL).</small>
          <span className="spacer" />
          <button className="primary" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
