// SQLCMD variables when only part of a script runs (a selection, or the
// statement at the cursor): the `:setvar` and `:on error` lines above it
// come along, so `$(name)` keeps its value.

const COMMAND = /^[ \t]*:(setvar|on[ \t]+error)\b.*$/gim;

/** `part`, which starts at offset `start` of `full`, with the SQLCMD lines above it in front. */
export function withSqlcmdContext(full: string, start: number, part: string): string {
  if (!part.includes("$(")) return part;
  const commands = full.slice(0, start).match(COMMAND);
  return commands ? `${commands.map((c) => c.trim()).join("\n")}\n${part}` : part;
}
