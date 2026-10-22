// The current settings, for parts of the UI that are not handed them as
// props (the result grid's exports read the CSV formula guard). App keeps
// it up to date.
import { useSyncExternalStore } from "react";
import type { Settings } from "./types";

let current: Settings | null = null;
const listeners = new Set<() => void>();

export function setPrefs(settings: Settings) {
  current = settings;
  listeners.forEach((l) => l());
}

export function prefs(): Settings | null {
  return current;
}

export function usePrefs(): Settings | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current,
  );
}
