// Monaco setup: local workers (no CDN), SQL languages, and the completion /
// formatting providers that call into Rust.
import * as monaco from "monaco-editor/editor";
// `monaco-editor/editor` is the bare API; editor features (suggest widget,
// formatting actions, find, folding, ...) are separate modules.
import "monaco-editor/features/register.all";
import "monaco-editor/languages/definitions/sql/register";
import "monaco-editor/languages/definitions/pgsql/register";
import "monaco-editor/languages/definitions/mysql/register";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import { api } from "./api";
import { ignoreOnLineEdit, nonOverlappingFixes } from "./problems";
import type { CompletionItem, CompletionKind, Diagnostic, Dialect, EditorArgs } from "./types";

self.MonacoEnvironment = { getWorker: () => new EditorWorker() };

export { monaco };

export const languageFor: Record<Dialect, string> = { mssql: "sql", postgres: "pgsql", mysql: "mysql" };

/** What the providers need to know about the tab owning a model. */
export interface ModelContext {
  dialect: Dialect;
  connectionId?: string;
  database?: string;
  server?: string;
}

const contexts = new Map<string, () => ModelContext>();

export function bindModelContext(model: monaco.editor.ITextModel, get: () => ModelContext) {
  contexts.set(model.uri.toString(), get);
}

export function unbindModelContext(model: monaco.editor.ITextModel) {
  contexts.delete(model.uri.toString());
  found.delete(model.uri.toString());
}

/** The latest analysis of each model (with quick fixes), for Problems and code actions. */
const found = new Map<string, Diagnostic[]>();

export function diagnosticsOf(model: monaco.editor.ITextModel): Diagnostic[] {
  return found.get(model.uri.toString()) ?? [];
}

/** Apply every quick fix that does not overlap another, as one undo step. */
export function applyFixes(editor: monaco.editor.IStandaloneCodeEditor, list: Diagnostic[]): number {
  const model = editor.getModel();
  if (!model) return 0;
  const fixes = nonOverlappingFixes(list);
  if (fixes.length === 0) return 0;
  const range = (start: number, end: number) => monaco.Range.fromPositions(model.getPositionAt(start), model.getPositionAt(end));
  editor.pushUndoStop();
  editor.executeEdits(
    "sqlotus-fix",
    fixes.map((f) => ({ range: range(f.start, f.end), text: f.text })),
  );
  editor.pushUndoStop();
  return fixes.length;
}

function contextOf(model: monaco.editor.ITextModel): ModelContext {
  return contexts.get(model.uri.toString())?.() ?? { dialect: "mssql" };
}

/** Arguments for the editor-intelligence commands. */
export function editorArgs(model: monaco.editor.ITextModel, position?: monaco.IPosition): EditorArgs {
  const ctx = contextOf(model);
  return {
    connectionId: ctx.connectionId,
    database: ctx.database,
    dialect: ctx.dialect,
    text: model.getValue(),
    offset: position ? model.getOffsetAt(position) : 0,
  };
}

const severityMap = {
  error: monaco.MarkerSeverity.Error,
  warning: monaco.MarkerSeverity.Warning,
  info: monaco.MarkerSeverity.Info,
} as const;

/** Run code analysis and show the results as squiggles. */
export async function updateDiagnostics(model: monaco.editor.ITextModel) {
  if (model.isDisposed()) return;
  const version = model.getVersionId();
  try {
    const list = await api.diagnostics(editorArgs(model));
    if (model.isDisposed() || model.getVersionId() !== version) return;
    found.set(model.uri.toString(), list);
    monaco.editor.setModelMarkers(
      model,
      "sqlotus",
      list.map((d) => {
        const s = model.getPositionAt(d.start);
        const e = model.getPositionAt(d.end);
        return {
          startLineNumber: s.lineNumber,
          startColumn: s.column,
          endLineNumber: e.lineNumber,
          endColumn: e.column,
          severity: severityMap[d.severity],
          message: d.message,
          code: d.code,
          source: "Sqlotus",
        };
      }),
    );
  } catch {
    // analysis errors must never disturb typing
  }
}

const K = monaco.languages.CompletionItemKind;
const kindMap: Record<CompletionKind, monaco.languages.CompletionItemKind> = {
  keyword: K.Keyword,
  function: K.Function,
  schema: K.Module,
  table: K.Struct,
  view: K.Interface,
  tableFunction: K.Method,
  scalarFunction: K.Function,
  procedure: K.Method,
  column: K.Field,
  alias: K.Variable,
  localTable: K.Struct,
  variable: K.Variable,
  snippet: K.Snippet,
  join: K.Reference,
  expansion: K.Value,
};

/** Words after which a space should open the suggestion list. */
const SPACE_TRIGGERS = new Set([
  "FROM", "JOIN", "UPDATE", "INTO", "EXEC", "EXECUTE", "TABLE", "ON", "APPLY", "CALL", "USING", "TRUNCATE",
]);

function toMonacoItem(
  model: monaco.editor.ITextModel,
  it: CompletionItem,
  range: monaco.IRange,
): monaco.languages.CompletionItem {
  let itemRange = range;
  if (it.replaceStart != null) {
    const start = model.getPositionAt(it.replaceStart);
    itemRange = { ...range, startLineNumber: start.lineNumber, startColumn: start.column };
  }
  return {
    label: { label: it.label, description: it.detail },
    kind: kindMap[it.kind] ?? K.Text,
    detail: it.detail,
    insertText: it.insertText,
    insertTextRules: it.isSnippet ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet : undefined,
    sortText: it.sortText,
    filterText: it.filterText ?? undefined,
    range: itemRange,
  };
}

export async function fetchCompletions(model: monaco.editor.ITextModel, position: monaco.Position) {
  const ctx = contextOf(model);
  const text = model.getValue();
  const offset = model.getOffsetAt(position);
  const r = await api.complete({ ...ctx, text, offset });
  const start = model.getPositionAt(r.replaceStart);
  const end = model.getPositionAt(Math.max(r.replaceEnd, offset));
  const range = { startLineNumber: start.lineNumber, startColumn: start.column, endLineNumber: end.lineNumber, endColumn: end.column };
  return { result: r, range };
}

let registered = false;

export function registerProviders() {
  if (registered) return;
  registered = true;
  for (const lang of Object.values(languageFor)) {
    monaco.languages.registerCompletionItemProvider(lang, {
      triggerCharacters: [".", " ", "@"],
      async provideCompletionItems(model, position, context) {
        if (context.triggerCharacter === " ") {
          const before = model.getValueInRange({
            startLineNumber: position.lineNumber,
            startColumn: 1,
            endLineNumber: position.lineNumber,
            endColumn: position.column,
          });
          const word = before.trimEnd().split(/\s+/).pop()?.toUpperCase() ?? "";
          if (!SPACE_TRIGGERS.has(word)) return { suggestions: [] };
        }
        try {
          const { result, range } = await fetchCompletions(model, position);
          return { suggestions: result.items.map((it) => toMonacoItem(model, it, range)), incomplete: false };
        } catch {
          return { suggestions: [] };
        }
      },
    });

    // the light bulb: the quick fix of a problem, and switching its rule off on that line
    monaco.languages.registerCodeActionProvider(lang, {
      provideCodeActions(model, _range, context) {
        const actions: monaco.languages.CodeAction[] = [];
        const list = diagnosticsOf(model);
        for (const marker of context.markers) {
          if (marker.source !== "Sqlotus") continue;
          const code = String(typeof marker.code === "object" ? marker.code.value : (marker.code ?? ""));
          const start = model.getOffsetAt({ lineNumber: marker.startLineNumber, column: marker.startColumn });
          const d = list.find((x) => x.code === code && x.start === start);
          if (d?.fix) {
            const f = d.fix;
            actions.push({
              title: f.title,
              kind: "quickfix",
              isPreferred: true,
              diagnostics: [marker],
              edit: {
                edits: [
                  {
                    resource: model.uri,
                    versionId: model.getVersionId(),
                    textEdit: { range: monaco.Range.fromPositions(model.getPositionAt(f.start), model.getPositionAt(f.end)), text: f.text },
                  },
                ],
              },
            });
          }
          const ignore = ignoreOnLineEdit(model.getLineContent(marker.startLineNumber), code);
          if (ignore) {
            actions.push({
              title: `Ignore ${code} on this line`,
              kind: "quickfix",
              diagnostics: [marker],
              edit: {
                edits: [
                  {
                    resource: model.uri,
                    versionId: model.getVersionId(),
                    textEdit: {
                      range: new monaco.Range(marker.startLineNumber, ignore.column, marker.startLineNumber, ignore.column),
                      text: ignore.text,
                    },
                  },
                ],
              },
            });
          }
        }
        return { actions, dispose: () => undefined };
      },
    });

    monaco.languages.registerHoverProvider(lang, {
      async provideHover(model, position) {
        try {
          const h = await api.hover(editorArgs(model, position));
          if (!h) return null;
          const s = model.getPositionAt(h.start);
          const e = model.getPositionAt(h.end);
          return {
            range: new monaco.Range(s.lineNumber, s.column, e.lineNumber, e.column),
            contents: [{ value: h.contents }],
          };
        } catch {
          return null;
        }
      },
    });

    monaco.languages.registerSignatureHelpProvider(lang, {
      signatureHelpTriggerCharacters: ["(", ",", " "],
      signatureHelpRetriggerCharacters: [","],
      async provideSignatureHelp(model, position) {
        try {
          const sig = await api.signatureHelp(editorArgs(model, position));
          if (!sig) return null;
          return {
            value: {
              signatures: [{ label: sig.label, parameters: sig.parameters.map((p) => ({ label: p })) }],
              activeSignature: 0,
              activeParameter: sig.activeParameter,
            },
            dispose() {},
          };
        } catch {
          return null;
        }
      },
    });

    // F2: rename a variable, table alias or CTE
    monaco.languages.registerRenameProvider(lang, {
      async resolveRenameLocation(model, position) {
        const t = await api.renameTarget(editorArgs(model, position)).catch(() => null);
        const off = model.getOffsetAt(position);
        const here = t?.ranges.find(([s, e]) => s <= off && off <= e);
        if (!t || !here) {
          return {
            range: new monaco.Range(position.lineNumber, position.column, position.lineNumber, position.column),
            text: "",
            rejectReason: "Only variables, table aliases and CTEs can be renamed",
          };
        }
        const a = model.getPositionAt(here[0]);
        const b = model.getPositionAt(here[1]);
        const range = new monaco.Range(a.lineNumber, a.column, b.lineNumber, b.column);
        return { range, text: model.getValueInRange(range) };
      },
      async provideRenameEdits(model, position, newName) {
        const t = await api.renameTarget(editorArgs(model, position));
        if (!t) return { edits: [], rejectReason: "Nothing to rename here" };
        const name = t.kind === "variable" && !newName.startsWith("@") ? `@${newName}` : newName;
        return {
          edits: t.ranges.map(([s, e]) => {
            const a = model.getPositionAt(s);
            const b = model.getPositionAt(e);
            return {
              resource: model.uri,
              versionId: model.getVersionId(),
              textEdit: { range: new monaco.Range(a.lineNumber, a.column, b.lineNumber, b.column), text: name },
            };
          }),
        };
      },
    });

    monaco.languages.registerDocumentFormattingEditProvider(lang, {
      async provideDocumentFormattingEdits(model) {
        const formatted = await api.formatSql(model.getValue(), contextOf(model).dialect);
        return [{ range: model.getFullModelRange(), text: formatted }];
      },
    });

    monaco.languages.registerDocumentRangeFormattingEditProvider(lang, {
      async provideDocumentRangeFormattingEdits(model, range) {
        const formatted = await api.formatSql(model.getValueInRange(range), contextOf(model).dialect);
        return [{ range, text: formatted }];
      },
    });
  }
}

/** `*` in a select list: after SELECT [DISTINCT] [TOP n], after a comma, or `alias.*`. */
const SELECT_STAR = /(\bselect\s+(distinct\s+)?(top\s*\(?\s*\d+\s*\)?\s+)?|,\s*|[\w\]"`]\.)\*$/i;

/**
 * Wildcard expansion: Tab right after `*` replaces it with the column list.
 * "no-tables" means a select-list `*` with no table in FROM yet.
 */
export async function expandWildcard(
  editor: monaco.editor.IStandaloneCodeEditor,
): Promise<"expanded" | "no-tables" | "none"> {
  const model = editor.getModel();
  const pos = editor.getPosition();
  if (!model || !pos || pos.column < 2) return "none";
  const before = model.getValueInRange({
    startLineNumber: Math.max(1, pos.lineNumber - 5),
    startColumn: 1,
    endLineNumber: pos.lineNumber,
    endColumn: pos.column,
  });
  if (!before.endsWith("*")) return "none";
  const { result, range } = await fetchCompletions(model, pos);
  const exp = result.items.find((i) => i.kind === "expansion");
  if (!exp || exp.replaceStart == null) return SELECT_STAR.test(before) ? "no-tables" : "none";
  const start = model.getPositionAt(exp.replaceStart);
  editor.executeEdits("expand-wildcard", [
    {
      range: new monaco.Range(start.lineNumber, start.column, range.endLineNumber, range.endColumn),
      text: exp.insertText,
    },
  ]);
  return "expanded";
}
