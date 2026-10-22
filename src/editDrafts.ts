// Pending row edits, kept per tab while another tab is shown. The result grid
// is one component for whichever tab is active, so its own state would go
// as soon as the user switched tabs. A draft belongs to one result (the same
// result set object): a new run of the tab leaves it stale, and it is
// dropped.
import type { ResultSet } from "./types";

interface Draft {
  rs: ResultSet;
  edit: unknown;
}

const drafts = new Map<string, Draft>();

/** Remember the edits of tab `tabId` on `rs`; null forgets them. */
export function keepDraft<T>(tabId: string, rs: ResultSet, edit: T | null) {
  if (edit) drafts.set(tabId, { rs, edit });
  else drafts.delete(tabId);
}

/** The edits of tab `tabId` on this very result, if any. */
export function takeDraft<T>(tabId: string, rs: ResultSet): T | null {
  const d = drafts.get(tabId);
  return d && d.rs === rs ? (d.edit as T) : null;
}

/** Forget a closed tab's edits. */
export function dropDraft(tabId: string) {
  drafts.delete(tabId);
}
