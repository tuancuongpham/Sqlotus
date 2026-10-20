import { dismiss, useToasts } from "../notify";

const ICON = { success: "✓", info: "ℹ", warning: "⚠", error: "✕", progress: "" } as const;

/** Stack of notifications in the bottom-right corner. */
export function Toaster() {
  const toasts = useToasts();
  return (
    <div className="toaster" role="status" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={`toast toast-${t.kind}`}>
          <span className="toast-icon">{t.kind === "progress" ? <span className="spinner" /> : ICON[t.kind]}</span>
          <div className="toast-body">
            <div className="toast-title">{t.title}</div>
            {t.message && <div className="toast-message">{t.message}</div>}
            {t.actions && t.actions.length > 0 && (
              <div className="toast-actions">
                {t.actions.map((a) => (
                  <button
                    key={a.label}
                    onClick={() => {
                      dismiss(t.id);
                      a.run();
                    }}
                  >
                    {a.label}
                  </button>
                ))}
              </div>
            )}
          </div>
          <button className="toast-close" onClick={() => dismiss(t.id)} title="Dismiss">
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
