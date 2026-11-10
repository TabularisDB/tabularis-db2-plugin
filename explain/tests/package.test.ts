import { readFile } from "node:fs/promises";

import { registrations, type RegisteredExplainParser } from "@tabularis/explain";
import { describe, expect, it } from "vitest";

import { db2ExplainParser, parseDb2Explain } from "../src/index";

const fixtureUrl = new URL("./fixtures/outer-join-index-range.json", import.meta.url);

describe("package entry points", () => {
  it("registers on ESM import and exports the direct parser API", async () => {
    expect(registrations).toEqual([db2ExplainParser]);
    expect(db2ExplainParser).toMatchObject({
      engine: "db2",
      format: "db2-explain-tables-json",
      label: "Db2 explain tables",
      parse: parseDb2Explain,
    });
    const fixture = await readFile(fixtureUrl, "utf8");
    expect(db2ExplainParser.sniff?.(fixture)).toBe(true);
    // The plugin sends the payload compact.
    expect(db2ExplainParser.sniff?.(JSON.stringify(JSON.parse(fixture)))).toBe(true);
    expect(db2ExplainParser.sniff?.('{"Plan": {}}')).toBe(false);
    expect(db2ExplainParser.sniff?.("<ShowPlanXML/>")).toBe(false);
  });

  it("builds an isolated IIFE descriptor without self-registration", async () => {
    const source = await readFile(new URL("../dist/index.iife.js", import.meta.url), "utf8");
    const registrationsBeforeEvaluation = registrations.length;
    const evaluate = new Function(
      "__TABULARIS_EXPLAIN__",
      `${source}\nreturn typeof __tabularis_explain_parser__ !== "undefined" ? __tabularis_explain_parser__ : null;`,
    );
    const raw = evaluate({}) as Record<string, unknown>;
    const descriptor = (raw.default ?? raw) as RegisteredExplainParser;

    expect(descriptor).toMatchObject({
      engine: "db2",
      format: "db2-explain-tables-json",
      label: "Db2 explain tables",
    });
    expect(descriptor.parse(await readFile(fixtureUrl, "utf8")).driver).toBe("db2");
    expect(registrations).toHaveLength(registrationsBeforeEvaluation);
  });
});
