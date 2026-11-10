import { readFile, writeFile } from "node:fs/promises";

import type { ExplainNode, ExplainPlan } from "@tabularis/explain";
import { describe, expect, it } from "vitest";

import type { Db2ExplainPayload } from "../src/payload";
import { buildPlanTree, parseDb2Explain, planningTimeMs, planText } from "../src/plan";

const fixtureNames = [
  "outer-join-index-range",
  "analyze-table-scan",
  "group-by-having",
  "union-exists",
  "update-by-key",
  "other-schema",
] as const;
const fixtureDirectory = new URL("./fixtures/", import.meta.url);

async function readFixture(name: string): Promise<string> {
  return readFile(new URL(`${name}.json`, fixtureDirectory), "utf8");
}

function flatten(node: ExplainNode): ExplainNode[] {
  return [node, ...node.children.flatMap(flatten)];
}

function shape(node: ExplainNode, depth = 0): string[] {
  const relation = node.relation === null ? "" : ` on ${node.relation}`;
  return [
    `${"  ".repeat(depth)}${node.node_type}${relation}`,
    ...node.children.flatMap((child) => shape(child, depth + 1)),
  ];
}

function find(root: ExplainNode, id: string): ExplainNode {
  const node = flatten(root).find((candidate) => candidate.id === id);
  if (!node) throw new Error(`no operator ${id}`);
  return node;
}

describe("Db2 payloads captured from Db2 12.1", () => {
  it.each(fixtureNames)("matches the committed golden for %s", async (name) => {
    const plan = parseDb2Explain(await readFixture(name));
    const goldenUrl = new URL(`expected/${name}.json`, fixtureDirectory);
    if (process.env.UPDATE_GOLDENS === "1") {
      await writeFile(goldenUrl, `${JSON.stringify(plan, null, 2)}\n`);
    }
    const expected = JSON.parse(await readFile(goldenUrl, "utf8")) as ExplainPlan;
    expect(plan).toEqual(expected);
  });

  it("draws tables and indexes as what an operator reads, not as operators", async () => {
    const plan = parseDb2Explain(await readFixture("outer-join-index-range"));
    expect(shape(plan.root)).toEqual([
      "RETURN",
      "  TBSCAN",
      "    SORT",
      "      HSJOIN",
      "        TBSCAN on DEPARTMENTS",
      "        FETCH on EMPLOYEES",
      "          IXSCAN on EMPLOYEES",
    ]);
    for (const node of flatten(plan.root)) {
      expect(node.id).not.toBe("-1");
    }
  });

  it("places predicates where Db2 applies them", async () => {
    const { root } = parseDb2Explain(await readFixture("outer-join-index-range"));
    const join = find(root, "4");
    expect(join.join_type).toBe("Right");
    expect(join.hash_condition).toBe("(Q1.DEPT_ID = Q3.DEPT_ID)");
    expect(find(root, "6").filter).toBe("(1000 < Q2.SALARY)");
    const indexScan = find(root, "7");
    expect(indexScan.index_condition).toBe("(Q2.EMP_ID <= 5) AND (2 <= Q2.EMP_ID)");
    expect(indexScan.extra.index).toMatch(/^SYSIBM\.SQL\d+$/);
    expect(find(root, "3").extra.SORTKEY).toBe("1: Q4.LAST_NAME(A)");
  });

  it("marks table scans of tables as full scans", async () => {
    const { root } = parseDb2Explain(await readFixture("outer-join-index-range"));
    expect(find(root, "5").extra.access_type).toBe("ALL");
    // The scan of the sorted rows reads a temporary table.
    expect(find(root, "2").extra.access_type).toBeUndefined();
  });

  it("reads Db2's compile time and, with Analyze, the run", async () => {
    const plan = parseDb2Explain(await readFixture("analyze-table-scan"));
    expect(plan.planning_time_ms).toBeGreaterThan(0);
    expect(plan.has_analyze_data).toBe(true);
    expect(plan.root.actual_rows).toBe(1100);
    expect(plan.root.actual_loops).toBe(1);
    expect(plan.execution_time_ms).toBe(plan.root.actual_time_ms);
    for (const node of flatten(plan.root).slice(1)) {
      expect(node.actual_rows).toBeNull();
    }

    const estimated = parseDb2Explain(await readFixture("outer-join-index-range"));
    expect(estimated.has_analyze_data).toBe(false);
    expect(estimated.execution_time_ms).toBeNull();
  });

  it("explains data changes", async () => {
    const { root } = parseDb2Explain(await readFixture("update-by-key"));
    expect(shape(root)).toEqual([
      "RETURN",
      "  UPDATE",
      "    FETCH on CRUD_TEST",
      "      IXSCAN on CRUD_TEST",
    ]);
  });

  it("qualifies objects of other schemas", async () => {
    const { root } = parseDb2Explain(await readFixture("other-schema"));
    expect(flatten(root).map((node) => node.relation).filter(Boolean)).toEqual([
      "Mixed Case.Order Details",
    ]);
  });
});

function payload(overrides: Partial<Db2ExplainPayload>): Db2ExplainPayload {
  return {
    db2_explain: 1,
    schema: "S",
    operators: [],
    streams: [],
    predicates: [],
    arguments: [],
    indexes: [],
    analyze: null,
    ...overrides,
  };
}

function operator(id: number, type: string) {
  return {
    id,
    type,
    total_cost: 1,
    io_cost: null,
    cpu_cost: null,
    first_row_cost: 0.5,
    re_total_cost: null,
    buffers: null,
  };
}

function stream(source: number, target: number, count = 1) {
  return {
    source_type: "O",
    source_id: source,
    target_id: target,
    object_schema: null,
    object_name: null,
    count,
  };
}

describe("buildPlanTree", () => {
  it("draws a shared input under each reader", () => {
    const root = buildPlanTree(
      payload({
        operators: [
          operator(1, "RETURN"),
          operator(2, "UNION"),
          operator(3, "TBSCAN"),
          operator(4, "TBSCAN"),
          operator(5, "TEMP"),
        ],
        streams: [stream(2, 1), stream(3, 2), stream(4, 2), stream(5, 3), stream(5, 4)],
      }),
    );
    expect(root && shape(root)).toEqual([
      "RETURN",
      "  UNION",
      "    TBSCAN",
      "      TEMP",
      "    TBSCAN",
      "      TEMP",
    ]);
  });

  it("puts the outer input of a join first", () => {
    const root = buildPlanTree(
      payload({
        operators: [operator(1, "NLJOIN"), operator(2, "IXSCAN"), operator(3, "TBSCAN")],
        streams: [stream(2, 1), stream(3, 1)],
        arguments: [
          { operator_id: 2, type: "JN INPUT", value: "INNER" },
          { operator_id: 3, type: "JN INPUT", value: "OUTER" },
        ],
        predicates: [{ operator_id: 1, how_applied: "JOIN", text: "(Q1.A = Q2.A)" }],
      }),
    );
    expect(root?.children.map((child) => child.id)).toEqual(["3", "2"]);
    expect(root?.join_type).toBe("Inner");
    // Only a hash join has a hash condition.
    expect(root?.hash_condition).toBeNull();
    expect(root?.extra.join_condition).toBe("(Q1.A = Q2.A)");
  });

  it("lists an equality used as start and stop once", () => {
    const root = buildPlanTree(
      payload({
        operators: [operator(1, "IXSCAN")],
        predicates: [
          { operator_id: 1, how_applied: "START", text: "(Q1.ID = 3)" },
          { operator_id: 1, how_applied: "STOP", text: "(Q1.ID = 3)" },
        ],
      }),
    );
    expect(root?.index_condition).toBe("(Q1.ID = 3)");
  });

  it("finds no root in a loop and none without operators", () => {
    expect(
      buildPlanTree(
        payload({
          operators: [operator(1, "RETURN"), operator(2, "TBSCAN")],
          streams: [stream(2, 1), stream(1, 2)],
        }),
      ),
    ).toBeNull();
    expect(buildPlanTree(payload({}))).toBeNull();
  });
});

describe("planningTimeMs", () => {
  it.each([
    ["        46 milliseconds", 46],
    ["2 seconds", 2000],
    ["500 microseconds", 0.5],
    ["soon", null],
  ])("reads %j", (value, expected) => {
    expect(
      planningTimeMs(payload({ arguments: [{ operator_id: 1, type: "PREPTIME", value }] })),
    ).toBe(expected);
  });
});

describe("parseDb2Explain", () => {
  it("refuses what is not a Db2 payload", () => {
    expect(() => parseDb2Explain("<xml/>")).toThrow(/not JSON/);
    expect(() => parseDb2Explain("[]")).toThrow(/version undefined/);
    expect(() => parseDb2Explain('{"db2_explain":2}')).toThrow(/version 2/);
    expect(() => parseDb2Explain('{"db2_explain":1}')).toThrow(/'operators'/);
    expect(() => parseDb2Explain(JSON.stringify(payload({})))).toThrow(/no operator/);
  });

  it("writes the plan as text for the raw view", async () => {
    const plan = parseDb2Explain(await readFixture("outer-join-index-range"));
    expect(plan.raw_output).toBe(planText(plan.root));
    expect(plan.raw_output).toMatch(/^1 RETURN \(cost=13\.54 rows=4\)\n/);
    expect(plan.raw_output).toContain("      4 HSJOIN (cost=13.54 rows=4)\n        Join: (Q1.DEPT_ID = Q3.DEPT_ID)\n");
    expect(plan.raw_output).toMatch(/ {10}7 IXSCAN on EMPLOYEES using SYSIBM\.SQL\d+ /);
  });
});


describe("db2exfmt reports", () => {
  it("rejects a report with missing operator details", async () => {
    const report = await readFile(new URL("./fixtures/db2exfmt-scan.txt", import.meta.url), "utf8");
    expect(() => parseDb2Explain(report.replace(/\n\s*2\) TBSCAN:[\s\S]*/, ""))).toThrow(/incomplete/);
  });
  it("parses a Db2 12.1 table scan report", async () => {
    const report = await readFile(new URL("./fixtures/db2exfmt-scan.txt", import.meta.url), "utf8");
    const plan = parseDb2Explain(report);
    expect(plan.root.node_type).toBe("RETURN");
    expect(plan.root.children[0].node_type).toBe("TBSCAN");
    expect(plan.root.children[0].relation).toBe("TEST_SCHEMA.EMPLOYEES");
    expect(plan.raw_output).toBe(report);
  });
  it("maps an index scan to its base table and key predicate", async () => {
    const report = await readFile(new URL("./fixtures/db2exfmt-index.txt", import.meta.url), "utf8");
    const node = flatten(parseDb2Explain(report).root).find(n => n.node_type === "IXSCAN")!;
    expect(node.relation).toBe("TEST_SCHEMA.EMPLOYEES");
    expect(node.index_condition).toContain("EMP_ID");
  });
  it("preserves the branches and predicates of a captured join", async () => {
    const report = await readFile(new URL("./fixtures/db2exfmt-join.txt", import.meta.url), "utf8");
    const plan = parseDb2Explain(report);
    expect(plan.root.children[0].children).toHaveLength(2);
    expect(JSON.stringify(plan.root)).toContain("DEPT_ID");
  });
});
