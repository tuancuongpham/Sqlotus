// The editor documents of the open tabs: a Monaco model per tab, the scroll
// and cursor position of the tabs not on screen, and the text last written
// to history, so an unchanged tab is not saved again.
import { bindModelContext, languageFor, monaco, unbindModelContext, type ModelContext } from "./monaco";
import type { Dialect } from "./types";

export class EditorDocuments {
  private readonly models = new Map<string, monaco.editor.ITextModel>();
  private readonly viewStates = new Map<string, monaco.editor.ICodeEditorViewState | null>();
  private readonly saved = new Map<string, string>();

  /** A document for tab `id`; `context` tells completion and analysis where the tab is connected. */
  open(id: string, text: string, dialect: Dialect, context: () => ModelContext): monaco.editor.ITextModel {
    const model = monaco.editor.createModel(text, languageFor[dialect]);
    this.models.set(id, model);
    this.saved.set(id, text);
    bindModelContext(model, context);
    return model;
  }

  get(id: string): monaco.editor.ITextModel | undefined {
    return this.models.get(id);
  }

  /** The tab's text, empty when it has no document. */
  text(id: string): string {
    return this.models.get(id)?.getValue() ?? "";
  }

  /** Note `text` as written to history; false when it already was. */
  noteSaved(id: string, text: string): boolean {
    if (this.saved.get(id) === text) return false;
    this.saved.set(id, text);
    return true;
  }

  /** Show tab `id` in `editor`, keeping the position of the tab it replaces. False when nothing changed. */
  show(editor: monaco.editor.IStandaloneCodeEditor, id: string): boolean {
    const model = this.models.get(id);
    const current = editor.getModel();
    if (!model || current === model) return false;
    const previous = [...this.models.entries()].find(([, m]) => m === current)?.[0];
    if (previous) this.viewStates.set(previous, editor.saveViewState());
    editor.setModel(model);
    const state = this.viewStates.get(id);
    if (state) editor.restoreViewState(state);
    return true;
  }

  close(id: string) {
    const model = this.models.get(id);
    if (model) {
      unbindModelContext(model);
      model.dispose();
    }
    this.models.delete(id);
    this.viewStates.delete(id);
    this.saved.delete(id);
  }
}
