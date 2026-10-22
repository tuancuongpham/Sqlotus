import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText } from "../api";
import { toast, update } from "../notify";
import type { Dialect, ScItem, ScLinkInfo, ScStatus } from "../types";
import { ObjectDiffView } from "./SchemaCompareDialog";

type Choice = "" | "mine" | "theirs";

const GROUPS: { state: ScItem["state"]; title: string; hint: string }[] = [
  { state: "local", title: "Changes in the database", hint: "commit them to the folder" },
  { state: "incoming", title: "Changes in the folder", hint: "get them into the database" },
  { state: "conflict", title: "Conflicts", hint: "both sides changed: choose one" },
];

function changeLabel(i: ScItem, status: Pick<ScStatus, "hasBase">): string {
  // without a common version nobody knows which side changed
  if (i.state === "conflict") return !status.hasBase ? "Differs (no common version yet)" : i.change === "changed" ? "Changed on both sides" : "Differs on both sides";
  const where = i.state === "local" ? "database" : "folder";
  // `change` compares the database with the folder: only in the database = new there / dropped from the folder
  const added = (i.change === "added") === (i.state === "local");
  return i.change === "changed" ? `Changed in the ${where}` : added ? `New in the ${where}` : `Dropped in the ${where}`;
}

const icon = (k: string) => (k === "table" ? "▦" : k === "view" ? "◫" : k === "trigger" ? "⚡" : "ƒ");

/**
 * Keep the schema of a database in a folder (a git repository): commit
 * database changes to it, and get the folder's changes into the database
 * through a script that opens in a tab for review.
 */
export function SourceControlDialog({
  connectionId,
  database,
  connectionName,
  dialect,
  onOpenScript,
  onClose,
}: {
  connectionId: string;
  database: string;
  connectionName: string;
  dialect: Dialect;
  onOpenScript: (title: string, text: string) => void;
  onClose: () => void;
}) {
  const [link, setLink] = useState<ScLinkInfo | null | undefined>(undefined);
  const [folderInput, setFolderInput] = useState("");
  const [initGit, setInitGit] = useState(true);
  const [status, setStatus] = useState<ScStatus | null>(null);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [choice, setChoice] = useState<Record<string, Choice>>({});
  const [current, setCurrent] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [log, setLog] = useState<string | null>(null);

  const refresh = async () => {
    setBusy("Comparing the database with the folder…");
    setError("");
    try {
      const s = await api.scStatus(connectionId, database);
      setStatus(s);
      setPicked(new Set(s.items.filter((i) => i.state !== "conflict").map((i) => i.key)));
      setChoice({});
      setCurrent(s.items[0]?.key ?? null);
      setLink(await api.scLinkInfo(connectionId, database));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy("");
    }
  };

  useEffect(() => {
    api
      .scLinkInfo(connectionId, database)
      .then((l) => {
        setLink(l);
        if (l) void refresh();
      })
      .catch((e) => {
        setLink(null);
        setError(errorText(e));
      });
  }, [connectionId, database]); // eslint-disable-line react-hooks/exhaustive-deps

  const doLink = async () => {
    setBusy("Linking…");
    setError("");
    try {
      const l = await api.scLink(connectionId, database, folderInput, initGit);
      setLink(l);
      toast.success("Database linked", l.folder);
      await refresh();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy("");
    }
  };

  const byState = useMemo(() => {
    const out: Record<ScItem["state"], ScItem[]> = { local: [], incoming: [], conflict: [] };
    for (const i of status?.items ?? []) out[i.state].push(i);
    return out;
  }, [status]);
  const commitKeys = (status?.items ?? [])
    .filter((i) => (i.state === "local" && picked.has(i.key)) || (i.state === "conflict" && choice[i.key] === "mine"))
    .map((i) => i.key);
  const latestKeys = (status?.items ?? [])
    .filter((i) => (i.state === "incoming" && picked.has(i.key)) || (i.state === "conflict" && choice[i.key] === "theirs"))
    .map((i) => i.key);
  const selected = status?.items.find((i) => i.key === current) ?? null;
  const toggle = (key: string, on: boolean) =>
    setPicked((p) => {
      const next = new Set(p);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });

  const commit = async () => {
    if (!status) return;
    const id = toast.progress("Committing…", `${commitKeys.length} object(s)`);
    try {
      const r = await api.scCommit(status.id, commitKeys, message);
      const files = `${r.written} file(s) written${r.removed ? `, ${r.removed} removed` : ""}.`;
      update(id, "success", r.commit ? `Committed ${r.commit}` : "Folder updated", r.note ? `${files} ${r.note}` : files);
      setMessage("");
      await refresh();
    } catch (e) {
      update(id, "error", "Could not commit", errorText(e));
      await refresh();
    }
  };

  const commitFiles = async () => {
    const id = toast.progress("Committing the folder's files…");
    try {
      const r = await api.scCommitFiles(connectionId, database, message);
      update(id, "success", r.commit ? `Committed ${r.commit}` : "Committed", `${r.written} file(s).`);
      setMessage("");
      await refresh();
    } catch (e) {
      update(id, "error", "Could not commit", errorText(e));
    }
  };

  const getLatest = async () => {
    if (!status) return;
    try {
      const s = await api.scGetLatestScript(status.id, latestKeys);
      onOpenScript(`Get latest ${database}`, s.script);
      const w = s.warnings.length;
      toast[w ? "warning" : "success"](
        `Get latest: ${s.statements} statement(s)`,
        `${w ? `${w} warning(s) at the top of the script. ` : ""}It opened in a tab connected to ${database}; review it, run it, then refresh here.`,
      );
      onClose();
    } catch (e) {
      toast.error("Could not build the script", errorText(e));
    }
  };

  const runGit = async (action: "pull" | "push" | "log") => {
    setBusy(action === "log" ? "Reading the history…" : `git ${action}…`);
    try {
      const out = await api.scGit(connectionId, database, action);
      if (action === "log") setLog(out);
      else {
        toast.success(`git ${action} done`, out || "Up to date.");
        if (action === "pull") await refresh();
      }
    } catch (e) {
      toast.error(`git ${action} failed`, errorText(e));
    } finally {
      setBusy("");
    }
  };

  const unlink = async () => {
    await api.scUnlink(connectionId, database);
    toast.info("Link removed", "The folder and its files are left as they are.");
    setLink(null);
    setStatus(null);
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal schema-compare-modal source-control-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">
          Source control — {connectionName} / {database}
        </div>
        {link === undefined ? (
          <div className="panel-empty">Loading…</div>
        ) : link === null ? (
          <div className="sc-setup settings-form">
            <p>
              Keep this database's schema in a folder — usually a git repository shared with your team. Sqlotus writes one
              <code> .sql</code> file per table, view, procedure, function and trigger, and <code>sqlotus-schema.json</code>, the snapshot
              it compares with. Commit sends database changes to the folder; Get latest builds a script that brings your team's
              changes into the database, for you to review and run.
            </p>
            <label>
              Folder (empty, or one that already holds a Sqlotus schema)
              <span className="sc-folder">
                <input value={folderInput} onChange={(e) => setFolderInput(e.target.value)} placeholder="C:\src\shop-db\schema" />
                <button
                  onClick={async () => {
                    const dir = await open({ directory: true, title: "Folder for the schema" });
                    if (typeof dir === "string") setFolderInput(dir);
                  }}
                >
                  Browse…
                </button>
              </span>
            </label>
            <label className="check">
              <input type="checkbox" checked={initGit} onChange={(e) => setInitGit(e.target.checked)} />
              Create a git repository there if it is not in one
            </label>
            <div>
              <button className="primary" onClick={() => void doLink()} disabled={!folderInput.trim() || !!busy}>
                Link
              </button>
            </div>
          </div>
        ) : (
          <>
            <div className="sc-head">
              <span title={link.folder}>📁 {link.folder}</span>
              {link.git ? <span className="sc-branch">⎇ {link.branch ?? "?"}</span> : <span className="status-error">not a git repository</span>}
              <span className="spacer" />
              <button onClick={() => void refresh()} disabled={!!busy}>
                Refresh
              </button>
              <button onClick={() => void runGit("pull")} disabled={!!busy || !link.git} title="git pull --ff-only">
                Pull
              </button>
              <button onClick={() => void runGit("push")} disabled={!!busy || !link.git} title="git push">
                Push
              </button>
              <button onClick={() => void runGit("log")} disabled={!!busy || !link.git}>
                History
              </button>
              <button onClick={() => void unlink()} disabled={!!busy} title="Forget the folder (its files stay)">
                Unlink
              </button>
            </div>
            {status && status.handEdited.length > 0 && (
              <div className="health-error sc-note" title={status.handEdited.join("\n")}>
                ⚠ {status.handEdited.length} file(s) in the folder do not match its snapshot ({status.handEdited.slice(0, 3).join(", ")}
                {status.handEdited.length > 3 ? ", …" : ""}). Sqlotus compares the snapshot, so change the database and commit instead of
                editing the files.
              </div>
            )}
            {status && status.uncommitted.length > 0 && (
              <div className="settings-hint sc-note sc-pending" title={status.uncommitted.join("\n")}>
                ⓘ {status.uncommitted.length} file(s) in the folder are not committed in git yet ({status.uncommitted.slice(0, 3).join(", ")}
                {status.uncommitted.length > 3 ? ", …" : ""}) — for example after a commit that git refused.{" "}
                <button onClick={() => void commitFiles()} disabled={!message.trim()} title="Write a commit message below first">
                  Commit these files
                </button>
              </div>
            )}
            {log !== null && (
              <pre className="sc-log">
                {log
                  ? log
                      .split("\n")
                      .map((l) => l.split("\t").join("   "))
                      .join("\n")
                  : "No commits yet."}
              </pre>
            )}
            {status && (
              <div className="compare-results">
                <div className="compare-list">
                  <div className="compare-summary">
                    <span>{status.inSync} in sync</span>
                    {status.firstCommit && <span>· nothing committed yet: every object is new to the folder</span>}
                    {!status.firstCommit && !status.hasBase && byState.conflict.length > 0 && (
                      <span>· first look at this folder: choose a side for each difference</span>
                    )}
                    {status.skipped.length > 0 && <span title={status.skipped.join(", ")}>· {status.skipped.length} not readable</span>}
                  </div>
                  {status.items.length === 0 && <div className="panel-empty">✓ The database and the folder are in sync.</div>}
                  {GROUPS.map((g) =>
                    byState[g.state].length === 0 ? null : (
                      <div key={g.state} className={`compare-group sc-${g.state}`}>
                        <div className="compare-group-head">
                          {g.state !== "conflict" && (
                            <input
                              type="checkbox"
                              checked={byState[g.state].every((i) => picked.has(i.key))}
                              onChange={(e) => byState[g.state].forEach((i) => toggle(i.key, e.target.checked))}
                            />
                          )}
                          {g.title} ({byState[g.state].length}) <small>— {g.hint}</small>
                        </div>
                        {byState[g.state].map((i) => (
                          <div key={i.key} className={"compare-item" + (i.key === current ? " active" : "")} onClick={() => setCurrent(i.key)}>
                            {g.state === "conflict" ? (
                              <select
                                value={choice[i.key] ?? ""}
                                onClick={(e) => e.stopPropagation()}
                                onChange={(e) => setChoice((c) => ({ ...c, [i.key]: e.target.value as Choice }))}
                              >
                                <option value="">Decide later</option>
                                <option value="mine">Keep the database version (commit)</option>
                                <option value="theirs">Take the folder version (get latest)</option>
                              </select>
                            ) : (
                              <input type="checkbox" checked={picked.has(i.key)} onClick={(e) => e.stopPropagation()} onChange={(e) => toggle(i.key, e.target.checked)} />
                            )}
                            <span className="compare-kind">{icon(i.kind)}</span>
                            <span className="compare-name">
                              {i.schema ? <small>{i.schema}.</small> : null}
                              {i.name}
                            </span>
                            <small className="sc-change">{changeLabel(i, status)}</small>
                          </div>
                        ))}
                      </div>
                    ),
                  )}
                </div>
                <div className="compare-detail">
                  {selected ? (
                    <>
                      <div className="compare-detail-head">
                        <b>
                          {selected.schema ? `${selected.schema}.` : ""}
                          {selected.name}
                        </b>
                        <span>{changeLabel(selected, status)}</span>
                      </div>
                      {selected.details.length > 0 && (
                        <ul className="compare-details">
                          {selected.details.map((x, n) => (
                            <li key={n}>{x}</li>
                          ))}
                        </ul>
                      )}
                      <div className="diff-labels">
                        <span>Folder (committed)</span>
                        <span>Database</span>
                      </div>
                      <ObjectDiffView diff={{ sourceText: selected.dbText, targetText: selected.folderText }} dialect={dialect} />
                    </>
                  ) : (
                    <div className="panel-empty">Select a change to see it side by side.</div>
                  )}
                </div>
              </div>
            )}
            {status && (
              <div className="sc-commit">
                <textarea
                  placeholder={status.git ? "Commit message (what changed and why)" : "Commit message (optional: the folder is not a git repository)"}
                  value={message}
                  onChange={(e) => setMessage(e.target.value)}
                  rows={2}
                />
                <button className="primary" onClick={() => void commit()} disabled={commitKeys.length === 0 || (status.git && !message.trim())}>
                  Commit ({commitKeys.length})
                </button>
                <button onClick={() => void getLatest()} disabled={latestKeys.length === 0} title="Build a script that brings these folder changes into the database">
                  Get latest ({latestKeys.length})
                </button>
              </div>
            )}
          </>
        )}
        {busy && <div className="settings-hint compare-note">{busy}</div>}
        {error && <div className="status-error compare-note">{error}</div>}
        <div className="modal-actions">
          <small>Get latest never changes the database here: its script opens in a tab for you to review and run.</small>
          <span className="spacer" />
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
