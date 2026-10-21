import { useMemo, useState } from "react";
import { groupProblems, type Problem, type ProblemGrouping } from "../problems";

const ICON: Record<Problem["severity"], string> = { error: "✕", warning: "⚠", info: "ℹ" };

/** Every code-analysis problem of the active tab, with jump, fix and export. */
export function ProblemsView({
  problems,
  onJump,
  onFix,
  onFixAll,
  onCopy,
}: {
  problems: Problem[];
  onJump: (p: Problem) => void;
  onFix: (p: Problem) => void;
  onFixAll: () => void;
  onCopy: () => void;
}) {
  const [by, setBy] = useState<ProblemGrouping>("none");
  const [filter, setFilter] = useState("");
  const groups = useMemo(() => groupProblems(problems, by, filter), [problems, by, filter]);
  const fixable = problems.filter((p) => p.fix).length;
  const shown = groups.reduce((n, g) => n + g.items.length, 0);
  return (
    <div className="problems">
      <div className="problems-bar">
        <input className="problems-filter" placeholder="Filter problems…" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <label>
          Group by
          <select value={by} onChange={(e) => setBy(e.target.value as ProblemGrouping)}>
            <option value="none">None</option>
            <option value="rule">Rule</option>
            <option value="severity">Severity</option>
          </select>
        </label>
        <span className="spacer" />
        <button onClick={onFixAll} disabled={fixable === 0} title="Apply every quick fix (one undo step)">
          Fix all ({fixable})
        </button>
        <button onClick={onCopy} disabled={problems.length === 0}>
          Copy as Markdown
        </button>
      </div>
      {problems.length === 0 ? (
        <div className="results-empty">✓ No problems found in this tab.</div>
      ) : shown === 0 ? (
        <div className="results-empty">No problem matches the filter.</div>
      ) : (
        <div className="problems-list">
          {groups.map((g) => (
            <div key={g.name || "all"}>
              {g.name && (
                <div className="problems-group">
                  {g.name} ({g.items.length})
                </div>
              )}
              {g.items.map((p) => (
                <div key={`${p.start}-${p.code}`} className={`problem-row severity-${p.severity}`} onClick={() => onJump(p)} title="Go to the problem">
                  <span className="problem-icon">{ICON[p.severity]}</span>
                  <span className="problem-code">{p.code}</span>
                  <span className="problem-message">{p.message}</span>
                  <span className="problem-pos">
                    {p.line}:{p.column}
                  </span>
                  {p.fix && (
                    <button
                      className="problem-fix"
                      onClick={(e) => {
                        e.stopPropagation();
                        onFix(p);
                      }}
                      title={p.fix.title}
                    >
                      Fix
                    </button>
                  )}
                </div>
              ))}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
