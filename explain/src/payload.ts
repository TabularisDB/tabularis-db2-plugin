/**
 * The payload the Db2 plugin's `explain_query` sends: the rows Db2's
 * `EXPLAIN` wrote into its explain tables for one statement, one list per
 * table, with values as Db2 gives them.
 *
 * Mirrors `Db2ExplainPayload` in the plugin's `src/models.rs`: change both
 * together.
 */
export interface Db2ExplainPayload {
  /** Version of this shape. Always the first key. */
  db2_explain: 1;
  /** The schema the statement was explained in. */
  schema: string;
  operators: Db2Operator[];
  streams: Db2Stream[];
  predicates: Db2Predicate[];
  arguments: Db2Argument[];
  indexes: Db2Index[];
  /** What running the statement showed, when Analyze was asked for. */
  analyze: Db2Analyze | null;
}

/** One `EXPLAIN_OPERATOR` row. */
export interface Db2Operator {
  id: number;
  type: string;
  total_cost: number | null;
  io_cost: number | null;
  cpu_cost: number | null;
  first_row_cost: number | null;
  re_total_cost: number | null;
  buffers: number | null;
}

/**
 * One `EXPLAIN_STREAM` row ending in an operator. `source_type` is `O` for
 * an operator and `D` for a data object (a table or an index, source id -1).
 */
export interface Db2Stream {
  source_type: string;
  source_id: number;
  target_id: number;
  object_schema: string | null;
  object_name: string | null;
  /** Estimated number of rows. */
  count: number | null;
}

/** One `EXPLAIN_PREDICATE` row. */
export interface Db2Predicate {
  operator_id: number;
  how_applied: string;
  text: string;
}

/** One `EXPLAIN_ARGUMENT` row. */
export interface Db2Argument {
  operator_id: number;
  type: string;
  value: string;
}

/** An index the plan reads, with its table. */
export interface Db2Index {
  schema: string;
  name: string;
  table_schema: string;
  table_name: string;
}

export interface Db2Analyze {
  /** Rows the statement returned, or changed when it returns none. */
  rows: number;
  elapsed_ms: number;
}

/** Whether `payload` starts like a Db2 payload. */
export function looksLikeDb2Payload(payload: string): boolean {
  return /^\s*\{\s*"db2_explain"\s*:/.test(payload.slice(0, 64));
}

/** Reads a payload, refusing anything that is not one this parser knows. */
export function readPayload(payload: string): Db2ExplainPayload {
  let value: unknown;
  try {
    value = JSON.parse(payload);
  } catch (error) {
    throw new Error(`Db2 EXPLAIN payload is not JSON: ${(error as Error).message}`);
  }
  if (typeof value !== "object" || value === null) {
    throw new Error("Db2 EXPLAIN payload is not an object");
  }
  const version = (value as { db2_explain?: unknown }).db2_explain;
  if (version !== 1) {
    throw new Error(
      `Db2 EXPLAIN payload version ${String(version)} is not supported by this parser`,
    );
  }
  const parsed = value as Db2ExplainPayload;
  for (const key of ["operators", "streams", "predicates", "arguments", "indexes"] as const) {
    if (!Array.isArray(parsed[key])) {
      throw new Error(`Db2 EXPLAIN payload has no '${key}' list`);
    }
  }
  return parsed;
}
