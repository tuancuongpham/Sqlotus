import { useCallback, useEffect, useState } from "react";
import { api, errorText } from "../api";
import { toast, update } from "../notify";
import type { ConnectionConfig, Diagnosis, Finding, Fix } from "../types";

const LEVEL_ICON: Record<Finding["level"], string> = { ok: "✓", info: "ℹ", warning: "⚠", error: "✕" };

interface Props {
  config: ConnectionConfig;
  password: string | null;
  error: string;
  onClose: () => void;
  /** Connect again, possibly with changed settings. */
  onRetry: (config: ConnectionConfig) => void;
}

/** Apply a settings-only fix to a connection config. */
function applyToConfig(cfg: ConnectionConfig, fix: Fix): ConnectionConfig {
  switch (fix.kind) {
    case "usePort":
      return { ...cfg, port: fix.port, host: cfg.host.split(",")[0].split("\\")[0] || "localhost" };
    case "useHost":
      return { ...cfg, host: fix.host, port: null };
    case "useWindowsAuth":
      return { ...cfg, auth: "windows" };
    case "trustCertificate":
      return { ...cfg, trustServerCertificate: true };
    case "useDatabase":
      return { ...cfg, database: fix.database };
    default:
      return cfg;
  }
}

/**
 * Explains a failed connection and offers one-click fixes: settings fixes
 * update the saved connection, machine fixes run elevated (UAC prompt).
 */
export function DoctorDialog({ config, password, error, onClose, onRetry }: Props) {
  const [cfg, setCfg] = useState(config);
  const [diagnosis, setDiagnosis] = useState<Diagnosis | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const run = useCallback(async (c: ConnectionConfig) => {
    setDiagnosis(null);
    try {
      setDiagnosis(await api.diagnoseConnection(c, error));
    } catch (e) {
      setDiagnosis({ summary: "Diagnosis failed", findings: [{ level: "error", title: errorText(e), detail: "", fix: null, fixLabel: null, needsAdmin: false }] });
    }
  }, [error]);

  useEffect(() => {
    void run(config);
  }, [config, run]);

  const fix = async (f: Finding) => {
    if (!f.fix) return;
    if (!f.needsAdmin) {
      const next = applyToConfig(cfg, f.fix);
      try {
        await api.saveConnection(next, null, false);
        setCfg(next);
        toast.success("Connection settings updated", `${f.fixLabel} — reconnecting…`);
        onRetry(next);
      } catch (e) {
        toast.error("Could not update the connection", errorText(e));
      }
      return;
    }
    setBusy(f.title);
    const id = toast.progress(`${f.fixLabel}…`, "Windows will ask for administrator permission.");
    try {
      const msg = await api.applyFix(f.fix, cfg, password);
      const works = msg.includes("works now");
      update(id, works ? "success" : "warning", works ? "Fixed" : "Fix applied", msg, works ? [{ label: "Connect", run: () => onRetry(cfg) }] : undefined);
      if (works) onRetry(cfg);
      else void run(cfg);
    } catch (e) {
      update(id, "error", "The fix could not be applied", errorText(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal doctor-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">🩺 Connection Doctor — {cfg.name || cfg.host}</div>
        <div className="doctor-body">
          {!diagnosis ? (
            <div className="panel-empty">
              <span className="spinner" /> Checking the server, services and network settings…
            </div>
          ) : (
            <>
              <div className="doctor-summary">{diagnosis.summary}</div>
              {diagnosis.findings.map((f, i) => (
                <div key={i} className={`finding finding-${f.level}`}>
                  <span className="finding-icon">{LEVEL_ICON[f.level]}</span>
                  <div className="finding-text">
                    <div className="finding-title">{f.title}</div>
                    {f.detail && <div className="finding-detail">{f.detail}</div>}
                  </div>
                  {f.fix && (
                    <button className={f.level === "error" ? "primary" : ""} disabled={busy !== null} onClick={() => fix(f)}>
                      {busy === f.title ? "Working…" : f.fixLabel}
                      {f.needsAdmin && " 🛡"}
                    </button>
                  )}
                </div>
              ))}
              <details className="doctor-error">
                <summary>Original error</summary>
                <pre>{error}</pre>
              </details>
            </>
          )}
        </div>
        <div className="modal-actions">
          <small>🛡 = needs administrator permission (Windows asks first)</small>
          <span className="spacer" />
          <button onClick={() => run(cfg)} disabled={!diagnosis}>
            Check again
          </button>
          <button onClick={() => onRetry(cfg)}>Try to connect</button>
          <button className="primary" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
