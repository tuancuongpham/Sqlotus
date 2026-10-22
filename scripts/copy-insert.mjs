// "Copy as INSERT" of src/export.ts for a result set in a JSON file
// ({ dialect, columns: [{ name, typeName }], rows }), printed to stdout.
// Used by crates/sqlotus-db/tests/copy_insert_live.rs to run the copies on
// real servers.
//
//   node scripts/copy-insert.mjs result.json
import { readFileSync } from "node:fs";
import { toInsert } from "../src/export.ts";

const input = JSON.parse(readFileSync(process.argv[2], "utf8"));
const rs = { columns: input.columns, rows: input.rows, truncated: false };
process.stdout.write(toInsert(rs, input.rows, input.dialect));
