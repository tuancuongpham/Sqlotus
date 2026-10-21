import { useEffect, useState, type ReactNode } from "react";
import type { Dialect, EditContext, ExecuteResult, ExplainOutput } from "../types";
import { PlanView } from "./PlanView";
import { ResultGrid } from "./ResultGrid";

const PROBLEMS = -3;
const PLAN = -2;
const MESSAGES = -1;

export function ResultsPanel({
  result,
  plan,
  showPlan,
  running,
  dialect,
  editContext,
  problems,
}: {
  result?: ExecuteResult;
  plan?: ExplainOutput;
  /** Show the plan tab (after Explain) instead of the results. */
  showPlan?: boolean;
  running: boolean;
  dialect?: Dialect;
  /** Lets the grid edit rows of a single-table result. */
  editContext?: EditContext;
  /** Code-analysis problems of the tab: count, worst severity, and the list view. */
  problems?: { count: number; worst?: "error" | "warning" | "info"; view: ReactNode };
}) {
  const sets = result?.batches.flatMap((b) => b.output.resultSets) ?? [];
  const messages = result?.batches.flatMap((b) => b.output.messages) ?? [];
  const failed = result?.batches.some((b) => b.output.failed) ?? false;
  const elapsed = result?.batches.reduce((a, b) => a + b.output.elapsedMs, 0) ?? 0;
  const [active, setActive] = useState<number>(0);

  useEffect(() => {
    setActive(sets.length > 0 && !failed ? 0 : MESSAGES);
  }, [result]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (plan && showPlan) setActive(PLAN);
  }, [plan, showPlan]);

  if (running) {
    return <div className="results-empty">Executing…</div>;
  }
  const problemCount = problems?.count ?? 0;
  if (!result && !plan && active !== PROBLEMS && problemCount === 0) {
    return <div className="results-empty">Run a query with F5 or Ctrl+E · estimated plan with Ctrl+L</div>;
  }
  const planWarnings = plan?.plans.reduce((n, p) => n + p.warnings.length + p.missingIndexes.length, 0) ?? 0;
  return (
    <div className="results">
      <div className="results-tabs">
        {sets.map((rs, i) => (
          <button key={i} className={active === i ? "active" : ""} onClick={() => setActive(i)}>
            Results {sets.length > 1 ? i + 1 : ""} <small>({rs.rows.length})</small>
          </button>
        ))}
        {result && (
          <button className={(active === MESSAGES ? "active" : "") + (failed ? " error" : "")} onClick={() => setActive(MESSAGES)}>
            Messages
          </button>
        )}
        {plan && (
          <button className={"plan-tab" + (active === PLAN ? " active" : "")} onClick={() => setActive(PLAN)}>
            Plan {planWarnings > 0 && <small className="plan-tab-warn">⚠ {planWarnings}</small>}
          </button>
        )}
        {problems && (
          <button className={"problems-tab" + (active === PROBLEMS ? " active" : "")} onClick={() => setActive(PROBLEMS)}>
            Problems <small className={problemCount ? `problems-count severity-${problems.worst}` : ""}>({problemCount})</small>
          </button>
        )}
        <span className="results-status">
          {active === PROBLEMS
            ? "Code analysis of this tab"
            : active === PLAN
            ? "Estimated plan — the query was not run"
            : result?.cancelled
              ? "Query cancelled"
              : result
                ? `${failed ? "Completed with errors" : "Query executed successfully"} · ${(elapsed / 1000).toFixed(3)}s`
                : ""}
        </span>
      </div>
      <div className="results-body">
        {active === PROBLEMS && problems ? (
          problems.view
        ) : !result && !plan ? (
          <div className="results-empty">Run a query with F5 or Ctrl+E · estimated plan with Ctrl+L</div>
        ) : active === PLAN && plan ? (
          <PlanView explain={plan} />
        ) : active >= 0 && sets[active] ? (
          <ResultGrid
            key={`${result?.batches.length}-${active}-${elapsed}`}
            rs={sets[active]}
            dialect={dialect}
            edit={sets.length === 1 ? editContext : undefined}
            label={`Results ${sets.length > 1 ? active + 1 : ""} · ${new Date().toLocaleTimeString()}`.replace("  ", " ")}
          />
        ) : result?.cancelled ? (
          <div className="results-empty">Query cancelled.</div>
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
