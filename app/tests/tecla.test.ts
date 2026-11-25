import { test } from "node:test";
import assert from "node:assert/strict";
import { tecla } from "../src/i18n.ts";

test("tecla troca todo ⌘ por Ctrl fora do mac e não mexe no mac", () => {
  assert.equal(tecla("⌘↓/⌘↑ anda", false), "Ctrl+↓/Ctrl+↑ anda");
  assert.equal(tecla("Laudo (⌥⌘I) · (⇧⌘G)", false), "Laudo (Ctrl+Alt+I) · (Ctrl+Shift+G)");
  assert.equal(tecla("⌘↓/⌘↑", true), "⌘↓/⌘↑");
});
