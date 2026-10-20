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
import type { CompletionItem, CompletionKind, Dialect, EditorArgs } from "./types";

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
    const found = await api.diagnostics(editorArgs(model));
    if (model.isDisposed() || model.getVersionId() !== version) return;
    monaco.editor.setModelMarkers(
      model,
      "sqlq",
      found.map((d) => {
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
          source: "SqlSupportQuery",
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

/**
 * Redgate-style wildcard expansion: Tab right after `*` replaces it with the
 * column list. Returns true when it handled the key.
 */
export async function expandWildcard(editor: monaco.editor.IStandaloneCodeEditor): Promise<boolean> {
  const model = editor.getModel();
  const pos = editor.getPosition();
  if (!model || !pos || pos.column < 2) return false;
  const prev = model.getValueInRange({
    startLineNumber: pos.lineNumber,
    startColumn: pos.column - 1,
    endLineNumber: pos.lineNumber,
    endColumn: pos.column,
  });
  if (prev !== "*") return false;
  const { result, range } = await fetchCompletions(model, pos);
  const exp = result.items.find((i) => i.kind === "expansion");
  if (!exp || exp.replaceStart == null) return false;
  const start = model.getPositionAt(exp.replaceStart);
  editor.executeEdits("expand-wildcard", [
    {
      range: new monaco.Range(start.lineNumber, start.column, range.endLineNumber, range.endColumn),
      text: exp.insertText,
    },
  ]);
  return true;
}
