import { registerExplainParser } from "@tabularis/explain";

import { db2ExplainParser } from "./parser";

registerExplainParser(db2ExplainParser);

export { db2ExplainParser } from "./parser";
export { buildPlanTree, parseDb2Explain, planText, planningTimeMs } from "./plan";
export type * from "./payload";
