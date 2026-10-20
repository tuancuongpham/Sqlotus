import { toast } from "../notify";
import type { ExplainOutput, PlanNode, StatementPlan } from "../types";

function fmtRows(n: number | null): string {
  if (n == null) return "";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M rows`;
  if (n >= 10_000) return `${Math.round(n / 1000)}k rows`;
  return `${Math.round(n * 10) / 10} row${n === 1 ? "" : "s"}`;
}

function Node({ node, depth }: { node: PlanNode; depth: number }) {
  const pct = node.costPercent ?? 0;
  const hot = pct >= 30;
  const body = (
    <div className={"plan-node" + (node.warnings.length ? " has-warning" : "") + (hot ? " hot" : "")}>
      <span className="plan-op">
        {node.warnings.length > 0 && <span className="plan-warn-icon" title={node.warnings.join("\n")}>⚠ </span>}
        {node.op}
      </span>
      {node.object && <span className="plan-object">{node.object}</span>}
      <span className="spacer" />
      <span className="plan-rows">{fmtRows(node.rows)}</span>
      {node.costPercent != null && (
        <span className="plan-cost" title="Share of the statement's estimated cost spent in this step">
          <span className="plan-bar" style={{ width: `${Math.min(100, pct)}%` }} />
          <b>{pct}%</b>
        </span>
      )}
    </div>
  );
  const detail = node.detail && <div className="plan-detail">{node.detail}</div>;
  const warnings = node.warnings.map((w, i) => (
    <div key={i} className="plan-node-warning">
      ⚠ {w}
    </div>
  ));
  if (node.children.length === 0) {
    return (
      <div className="plan-leaf" style={{ marginLeft: depth ? 16 : 0 }}>
        {body}
        {detail}
        {warnings}
      </div>
    );
  }
  return (
    <details open className="plan-branch" style={{ marginLeft: depth ? 16 : 0 }}>
      <summary>{body}</summary>
      {detail}
      {warnings}
      {node.children.map((c, i) => (
        <Node key={i} node={c} depth={depth + 1} />
      ))}
    </details>
  );
}

function Statement({ plan, index }: { plan: StatementPlan; index: number }) {
  return (
    <div className="plan-statement">
      <div className="plan-head">
        <b>Statement {index + 1}</b>
        {plan.cost != null && <span className="plan-total">estimated cost {Math.round(plan.cost * 1000) / 1000}</span>}
        <code title={plan.statement}>{plan.statement.replace(/\s+/g, " ").slice(0, 160)}</code>
      </div>
      {plan.warnings.length > 0 && (
        <ul className="plan-warnings">
          {plan.warnings.map((w, i) => (
            <li key={i}>⚠ {w}</li>
          ))}
        </ul>
      )}
      {plan.missingIndexes.map((m, i) => (
        <div key={i} className="plan-missing-index">
          <span>
            💡 Missing index on <b>{m.table}</b>
            {m.impact != null && <> — estimated {Math.round(m.impact)}% faster</>}
          </span>
          <code>{m.createStatement}</code>
          <button
            onClick={() =>
              void navigator.clipboard.writeText(m.createStatement).then(
                () => toast.success("CREATE INDEX copied", "Review the name and columns before running it."),
                (e) => toast.error("Copy failed", String(e)),
              )
            }
          >
            Copy
          </button>
        </div>
      ))}
      {plan.root ? <Node node={plan.root} depth={0} /> : <div className="panel-empty">No plan for this statement.</div>}
    </div>
  );
}

/** Estimated execution plan as an expandable tree, with warnings first. */
export function PlanView({ explain }: { explain: ExplainOutput }) {
  return (
    <div className="plan-view">
      {explain.errors.map((e, i) => (
        <div key={i} className="msg-error plan-error">
          {e}
        </div>
      ))}
      {explain.plans.length === 0 && explain.errors.length === 0 && (
        <div className="panel-empty">No statement to explain (only SELECT, INSERT, UPDATE, DELETE and MERGE have plans).</div>
      )}
      {explain.plans.map((p, i) => (
        <Statement key={i} plan={p} index={i} />
      ))}
    </div>
  );
}
