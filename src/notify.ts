// App-wide notifications: every action and setting change tells the user
// what happened (and what to do next when something fails).
import { useSyncExternalStore } from "react";

export type ToastKind = "success" | "info" | "warning" | "error" | "progress";

export interface ToastAction {
  label: string;
  run: () => void;
}

export interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  message?: string;
  actions?: ToastAction[];
}

let toasts: Toast[] = [];
let nextId = 1;
const listeners = new Set<() => void>();
const timers = new Map<number, number>();

const emit = () => listeners.forEach((l) => l());

/** How long each kind stays on screen (ms); errors and progress stay until closed. */
const DURATION: Record<ToastKind, number | null> = {
  success: 3500,
  info: 4500,
  warning: 8000,
  error: null,
  progress: null,
};

export function dismiss(id: number) {
  window.clearTimeout(timers.get(id));
  timers.delete(id);
  toasts = toasts.filter((t) => t.id !== id);
  emit();
}

function schedule(t: Toast) {
  window.clearTimeout(timers.get(t.id));
  const ms = DURATION[t.kind];
  if (ms != null) timers.set(t.id, window.setTimeout(() => dismiss(t.id), ms));
}

export function notify(kind: ToastKind, title: string, message?: string, actions?: ToastAction[]): number {
  const t: Toast = { id: nextId++, kind, title, message, actions };
  toasts = [...toasts, t].slice(-6);
  schedule(t);
  emit();
  return t.id;
}

/** Replace an existing toast (e.g. progress → success). */
export function update(id: number, kind: ToastKind, title: string, message?: string, actions?: ToastAction[]) {
  if (!toasts.some((t) => t.id === id)) return notify(kind, title, message, actions);
  toasts = toasts.map((t) => (t.id === id ? { id, kind, title, message, actions } : t));
  schedule({ id, kind, title, message, actions });
  emit();
  return id;
}

export const toast = {
  success: (title: string, message?: string, actions?: ToastAction[]) => notify("success", title, message, actions),
  info: (title: string, message?: string, actions?: ToastAction[]) => notify("info", title, message, actions),
  warning: (title: string, message?: string, actions?: ToastAction[]) => notify("warning", title, message, actions),
  error: (title: string, message?: string, actions?: ToastAction[]) => notify("error", title, message, actions),
  progress: (title: string, message?: string) => notify("progress", title, message),
};

export function useToasts(): Toast[] {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => toasts,
  );
}
