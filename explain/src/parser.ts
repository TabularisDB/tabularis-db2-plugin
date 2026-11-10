import type { RegisteredExplainParser } from "@tabularis/explain";

import { looksLikeDb2Payload } from "./payload";
import { parseDb2Explain } from "./plan";

/**
 * Db2 parser descriptor consumed by both package and plugin loaders. The
 * engine and format match the plugin's `explain_query` answer and the
 * `explain_parsers` entry of its `.tabularium`.
 */
export const db2ExplainParser: RegisteredExplainParser = {
  engine: "db2",
  format: "db2-explain-tables-json",
  label: "Db2 explain tables",
  parse: parseDb2Explain,
  sniff: looksLikeDb2Payload,
};
