import { useSyncExternalStore } from "react";
import type { ResultSet } from "./types";

/** A result set kept aside to compare a later result with. */
export interface PinnedResult {
  label: string;
  rs: ResultSet;
  at: number;
}

let pinned: PinnedResult | null = null;
const listeners = new Set<() => void>();

export function pin(label: string, rs: ResultSet) {
  pinned = { label, rs, at: Date.now() };
  listeners.forEach((l) => l());
}

export function unpin() {
  pinned = null;
  listeners.forEach((l) => l());
}

export function usePinned(): PinnedResult | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => pinned,
  );
}
