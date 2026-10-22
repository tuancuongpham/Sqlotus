// Unit tests of src/sqlcmd.ts (Node runs TypeScript directly).
import assert from "node:assert/strict";
import { test } from "node:test";
import { withSqlcmdContext } from "../src/sqlcmd.ts";

test("a part of a script keeps the SQLCMD variables above it", () => {
  const full = ":setvar Schema dbo\n:On Error Exit\nSELECT 1\nGO\n  :setvar Table Orders\nSELECT * FROM $(Schema).$(Table)\n:setvar Later x";
  const start = full.indexOf("SELECT *");
  const part = "SELECT * FROM $(Schema).$(Table)";
  assert.equal(withSqlcmdContext(full, start, part), ":setvar Schema dbo\n:On Error Exit\n:setvar Table Orders\n" + part);
  // nothing to add when the part uses no variable, or there are no commands
  assert.equal(withSqlcmdContext(full, full.indexOf("SELECT 1"), "SELECT 1"), "SELECT 1");
  assert.equal(withSqlcmdContext("SELECT '$(x)'", 0, "SELECT '$(x)'"), "SELECT '$(x)'");
});
