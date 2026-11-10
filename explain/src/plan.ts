/**
 * Builds the plan tree Tabularis draws from the explain-table rows.
 *
 * `EXPLAIN_OPERATOR` lists the operators (`RETURN`, `TBSCAN`, `HSJOIN`...),
 * `EXPLAIN_STREAM` the rows flowing between them and from tables and
 * indexes into them, `EXPLAIN_PREDICATE` where each predicate is applied
 * and `EXPLAIN_ARGUMENT` the settings of each operator.
 */

import { readDb2exfmt } from "./db2exfmt";
import type { ExplainNode, ExplainPlan } from "@tabularis/explain";

import {
  readPayload,
  type Db2Argument,
  type Db2ExplainPayload,
  type Db2Operator,
} from "./payload";

/** Names a table or index by name alone in the statement's schema. */
function objectLabel(schema: string, name: string, currentSchema: string): string {
  return schema === currentSchema ? name : `${schema}.${name}`;
}

/**
 * Joins predicates applied the same way, once each: Db2 lists an equality
 * on an index both as the start and as the stop of the scan.
 */
function joinPredicates(texts: string[]): string | null {
  const unique = [...new Set(texts)];
  return unique.length > 0 ? unique.join(" AND ") : null;
}

/**
 * The join type of a join operator from its `OUTERJN` argument: Db2 names
 * the side whose rows are all kept, as seen from the operator's own outer
 * and inner input.
 */
function joinType(operatorType: string, args: Map<string, string[]>): string | null {
  if (!operatorType.endsWith("JOIN")) {
    return null;
  }
  const outerJoin = args.get("OUTERJN")?.[0]?.trim().toUpperCase() ?? "";
  switch (outerJoin) {
    case "":
      return "Inner";
    case "LEFT":
      return "Left";
    case "RIGHT":
      return "Right";
    case "FULL":
      return "Full";
    default:
      return outerJoin;
  }
}

/** Reads `PREPTIME` ("46 milliseconds"), the time Db2 took to compile. */
export function planningTimeMs(payload: Db2ExplainPayload): number | null {
  const prepTime = payload.arguments.find((argument) => argument.type === "PREPTIME");
  if (!prepTime) {
    return null;
  }
  const [amount, unit = ""] = prepTime.value.trim().split(/\s+/);
  const value = Number(amount);
  if (!Number.isFinite(value)) {
    return null;
  }
  if (unit.startsWith("milli")) return value;
  if (unit.startsWith("micro")) return value / 1000;
  if (unit.startsWith("second")) return value * 1000;
  return null;
}

const PLACED_PREDICATES = new Set(["START", "STOP", "SARG", "RESID", "JOIN"]);

/** One operator as a plan node, without its children. */
function operatorNode(operator: Db2Operator, payload: Db2ExplainPayload): ExplainNode {
  const id = operator.id;
  const node: ExplainNode = {
    id: String(id),
    node_type: operator.type.trim(),
    relation: null,
    startup_cost: operator.first_row_cost,
    total_cost: operator.total_cost,
    plan_rows: null,
    actual_rows: null,
    actual_time_ms: null,
    actual_loops: null,
    buffers_hit: null,
    buffers_read: null,
    filter: null,
    index_condition: null,
    join_type: null,
    hash_condition: null,
    extra: {},
    children: [],
  };

  const args = new Map<string, string[]>();
  for (const argument of payload.arguments.filter((a) => a.operator_id === id)) {
    const key = argument.type.trim();
    args.set(key, [...(args.get(key) ?? []), argument.value.trim()]);
  }

  // The table this operator reads, directly or through an index.
  const current = payload.schema;
  for (const stream of payload.streams) {
    if (stream.target_id !== id || stream.source_type !== "D") {
      continue;
    }
    const schema = stream.object_schema ?? "";
    const name = stream.object_name ?? "";
    const index = payload.indexes.find(
      (candidate) => candidate.schema === schema && candidate.name === name,
    );
    if (index) {
      node.relation = objectLabel(index.table_schema, index.table_name, current);
      node.extra.index = objectLabel(schema, name, current);
    } else {
      node.relation = objectLabel(schema, name, current);
      if (node.node_type === "TBSCAN") {
        // Read by the explain view as a full scan of a table.
        node.extra.access_type = "ALL";
      }
    }
  }

  // Estimated rows: what this operator passes on, or for the top operator,
  // which passes nothing on, what it receives.
  const output = payload.streams.find(
    (stream) => stream.source_type === "O" && stream.source_id === id,
  );
  if (output && output.count !== null) {
    node.plan_rows = output.count;
  } else {
    const inputs = payload.streams
      .filter((stream) => stream.target_id === id && stream.source_type === "O")
      .map((stream) => stream.count)
      .filter((count): count is number => count !== null);
    node.plan_rows = inputs.length > 0 ? Math.max(...inputs) : null;
  }

  const predicates = payload.predicates.filter((predicate) => predicate.operator_id === id);
  const applied = (...kinds: string[]) =>
    joinPredicates(
      predicates
        .filter((predicate) => kinds.includes(predicate.how_applied.trim()))
        .map((predicate) => predicate.text.trim()),
    );
  node.index_condition = applied("START", "STOP");
  node.filter = applied("SARG", "RESID");
  const joinCondition = applied("JOIN");
  if (node.node_type === "HSJOIN") {
    node.hash_condition = joinCondition;
  } else if (joinCondition !== null) {
    node.extra.join_condition = joinCondition;
  }
  for (const predicate of predicates) {
    const how = predicate.how_applied.trim();
    if (!PLACED_PREDICATES.has(how)) {
      node.extra[`${how.toLowerCase()} predicate`] = predicate.text.trim();
    }
  }

  node.join_type = joinType(node.node_type, args);

  const costs: Array<[string, number | null]> = [
    ["io_cost", operator.io_cost],
    ["cpu_cost", operator.cpu_cost],
    ["re_total_cost", operator.re_total_cost],
    ["buffers", operator.buffers],
  ];
  for (const [key, value] of costs) {
    if (value !== null) {
      node.extra[key] = value;
    }
  }
  for (const [key, values] of [...args.entries()].sort(([a], [b]) => a.localeCompare(b))) {
    node.extra[key] = values.join(", ");
  }
  return node;
}

/** The outer input of a join first, as `db2exfmt` draws it, then by id. */
function inputOrder(args: Db2Argument[], source: number): [number, number] {
  const side = args.find(
    (argument) => argument.operator_id === source && argument.type.trim() === "JN INPUT",
  )?.value.trim();
  return [side === "OUTER" ? 0 : side === "INNER" ? 1 : 2, source];
}

function buildNode(
  operator: Db2Operator,
  payload: Db2ExplainPayload,
  byId: Map<number, Db2Operator>,
  path: number[],
): ExplainNode {
  const node = operatorNode(operator, payload);
  const inputs = [
    ...new Set(
      payload.streams
        .filter((stream) => stream.target_id === operator.id && stream.source_type === "O")
        .map((stream) => stream.source_id),
    ),
  ].sort((a, b) => {
    const [sideA, idA] = inputOrder(payload.arguments, a);
    const [sideB, idB] = inputOrder(payload.arguments, b);
    return sideA - sideB || idA - idB;
  });

  for (const source of inputs) {
    // A temporary table read by two operators feeds both and is drawn under
    // each. A loop would be a corrupt plan: stop there.
    const input = byId.get(source);
    if (input && !path.includes(source)) {
      node.children.push(buildNode(input, payload, byId, [...path, operator.id]));
    }
  }
  return node;
}

/**
 * The plan tree, rooted at the operator that passes its rows to no other
 * one (`RETURN` for a query). `null` when there is no such operator.
 */
export function buildPlanTree(payload: Db2ExplainPayload): ExplainNode | null {
  const byId = new Map(payload.operators.map((operator) => [operator.id, operator]));
  const hasConsumer = (id: number) =>
    payload.streams.some(
      (stream) => stream.source_type === "O" && stream.source_id === id && byId.has(stream.target_id),
    );
  const root = payload.operators
    .filter((operator) => !hasConsumer(operator.id))
    .sort((a, b) => a.id - b.id)[0];
  return root ? buildNode(root, payload, byId, []) : null;
}

/** The plan as indented text, one operator per line, for the raw view. */
export function planText(root: ExplainNode): string {
  const lines: string[] = [];
  const write = (node: ExplainNode, depth: number) => {
    const indent = "  ".repeat(depth);
    let line = `${indent}${node.id} ${node.node_type}`;
    if (node.relation !== null) line += ` on ${node.relation}`;
    if (typeof node.extra.index === "string") line += ` using ${node.extra.index}`;
    if (node.total_cost !== null) {
      line += ` (cost=${node.total_cost.toFixed(2)}`;
      if (node.plan_rows !== null) line += ` rows=${Math.round(node.plan_rows)}`;
      line += ")";
    }
    lines.push(line);
    const details: Array<[string, string | null]> = [
      ["Join", node.hash_condition],
      ["Index", node.index_condition],
      ["Filter", node.filter],
    ];
    for (const [label, value] of details) {
      if (value !== null) lines.push(`${indent}  ${label}: ${value}`);
    }
    for (const child of node.children) write(child, depth + 1);
  };
  write(root, 0);
  return `${lines.join("\n")}\n`;
}

/**
 * Parses the Db2 plugin's payload into a plan. With Analyze, the statement
 * ran: the top operator carries its real row count and elapsed time, and
 * the operators below keep their estimates.
 */
export function parseDb2Explain(rawPayload: string): ExplainPlan {
  const textReport = !/^\s*\{/.test(rawPayload) && /Explain Table Format Tool/i.test(rawPayload);
  const payload = textReport ? readDb2exfmt(rawPayload) : readPayload(rawPayload);
  const root = buildPlanTree(payload);
  if (root === null) {
    throw new Error("Db2 EXPLAIN payload has no operator to start the plan from");
  }
  if (payload.analyze) {
    root.actual_rows = payload.analyze.rows;
    root.actual_time_ms = payload.analyze.elapsed_ms;
    root.actual_loops = 1;
  }
  return {
    root,
    planning_time_ms: planningTimeMs(payload),
    execution_time_ms: payload.analyze?.elapsed_ms ?? null,
    original_query: "",
    driver: "db2",
    has_analyze_data: payload.analyze !== null,
    raw_output: textReport ? rawPayload : planText(root),
  };
}
