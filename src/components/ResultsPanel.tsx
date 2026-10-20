import { useEffect, useState } from "react";
import type { Dialect, ExecuteResult } from "../types";
import { ResultGrid } from "./ResultGrid";

export function ResultsPanel({ result, running, dialect }: { result?: ExecuteResult; running: boolean; dialect?: Dialect }) {
  const sets = result?.batches.flatMap((b) => b.output.resultSets) ?? [];
  const messages = result?.batches.flatMap((b) => b.output.messages) ?? [];
  const failed = result?.batches.some((b) => b.output.failed) ?? false;
  const elapsed = result?.batches.reduce((a, b) => a + b.output.elapsedMs, 0) ?? 0;
  const [active, setActive] = useState<number>(0); // -1 = messages

  useEffect(() => {
    setActive(sets.length > 0 && !failed ? 0 : -1);
  }, [result]); // eslint-disable-line react-hooks/exhaustive-deps

  if (running) {
    return <div className="results-empty">Executing…</div>;
  }
  if (!result) {
    return <div className="results-empty">Run a query with F5 or Ctrl+E</div>;
  }
  if (result.cancelled) {
    return <div className="results-empty">Query cancelled.</div>;
  }
  return (
    <div className="results">
      <div className="results-tabs">
        {sets.map((rs, i) => (
          <button key={i} className={active === i ? "active" : ""} onClick={() => setActive(i)}>
            Results {sets.length > 1 ? i + 1 : ""} <small>({rs.rows.length})</small>
          </button>
        ))}
        <button className={(active === -1 ? "active" : "") + (failed ? " error" : "")} onClick={() => setActive(-1)}>
          Messages
        </button>
        <span className="results-status">
          {failed ? "Completed with errors" : "Query executed successfully"} · {(elapsed / 1000).toFixed(3)}s
        </span>
      </div>
      <div className="results-body">
        {active >= 0 && sets[active] ? (
          <ResultGrid key={`${result.batches.length}-${active}-${elapsed}`} rs={sets[active]} dialect={dialect} />
        ) : (
          <pre className="messages">
            {messages.map((m, i) => (
              <div key={i} className={m.kind === "error" ? "msg-error" : ""}>
                {m.text}
              </div>
            ))}
          </pre>
        )}
      </div>
    </div>
  );
}
