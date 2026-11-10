import type { Db2ExplainPayload, Db2Operator } from "./payload";

/** Read the operator details and input streams from a single db2exfmt report. */
export function readDb2exfmt(text: string): Db2ExplainPayload {
  if (!/IBM Db2 Explain Table Format Tool|DB2 Explain Table Format Tool/i.test(text) || !/^Plan Details:\s*$/m.test(text)) {
    throw new Error("Db2 report must contain db2exfmt Plan Details");
  }
  if ((text.match(/^[-*]+\s*STATEMENT\s+\d+/gm) ?? []).length > 1) {
    throw new Error("Db2 report contains multiple statements; format a single statement");
  }
  const details = text.split(/^Plan Details:\s*$/m)[1].split(/^Objects Used in Access Plan:\s*$/m)[0];
  const matches = [...details.matchAll(/^\s*(\d+)\)\s+([A-Z][A-Z0-9_ ]*?)\s*:\s*\([^\n]*\)\s*$/gm)];
  const payload: Db2ExplainPayload = { db2_explain: 1, schema: '', operators: [], streams: [], predicates: [], arguments: [], indexes: [], analyze: null };
  const number = (s: string | undefined) => s && Number.isFinite(Number(s.trim())) ? Number(s.trim()) : null;
  for (let i = 0; i < matches.length; i++) {
    const m = matches[i];
    const id = Number(m[1]);
    const body = details.slice(m.index! + m[0].length, matches[i + 1]?.index ?? details.length);
    const field = (name: string) => number(body.match(new RegExp('^\\s*' + name + ':\\s*([^\\n]+)', 'm'))?.[1]);
    const operator: Db2Operator = { id, type: m[2], total_cost: field('Cumulative Total Cost'), cpu_cost: field('Cumulative CPU Cost'), io_cost: field('Cumulative I/O Cost'), first_row_cost: field('Cumulative First Row Cost'), re_total_cost: field('Cumulative Re-Total Cost'), buffers: field('Estimated Bufferpool Buffers') };
    payload.operators.push(operator);
    const args = body.match(/\n\s*Arguments:\s*\n[^\n]*\n([\s\S]*?)(?=\n\s*(?:Predicates|Input Streams|Output Streams|Table Cardinality Model Predicates):|$)/)?.[1] ?? '';
    for (const arg of args.matchAll(/^\s*([A-Z][A-Z0-9_ ]*?)\s*:\s*\([^\n]*\)\s*\n\s*([^\n]+)/gm)) payload.arguments.push({ operator_id: id, type: arg[1], value: arg[2].trim() });
    const input = body.match(/\n\s*Input Streams:\s*\n[^\n]*\n([\s\S]*?)(?=\n\s*Output Streams:|$)/)?.[1] ?? '';
    const streams = [...input.matchAll(/^\s*\d+\) From (Operator #\d+|Object [^\n]+)\s*$/gm)];
    for (let j = 0; j < streams.length; j++) {
      const stream = streams[j];
      const chunk = input.slice(stream.index! + stream[0].length, streams[j + 1]?.index ?? input.length);
      const rows = number(chunk.match(/Estimated number of rows:\s*([^\n]+)/)?.[1]);
      if (stream[1].startsWith('Operator #')) payload.streams.push({ source_type: 'O', source_id: Number(stream[1].slice(10)), target_id: id, object_schema: null, object_name: null, count: rows });
      else {
        const object = stream[1].slice(7).trim();
        const dot = object.indexOf('.');
        if (dot < 1) throw new Error('Unsupported Db2 object name in report');
        payload.streams.push({ source_type: 'D', source_id: -1, target_id: id, object_schema: object.slice(0, dot), object_name: object.slice(dot + 1), count: rows });
      }
    }
    const preds = body.match(/\n\s*Predicates:\s*\n[^\n]*\n([\s\S]*?)(?=\n\s*(?:Input Streams|Output Streams|Table Cardinality Model Predicates):|$)/)?.[1] ?? '';
    for (const pred of preds.matchAll(/\d+\)\s*([^\n]+)[\s\S]*?Predicate Text:\s*\n[^\n]*\n\s*([^\n]+)/g)) {
      const kind = /start key/i.test(pred[1]) ? 'START' : /stop key/i.test(pred[1]) ? 'STOP' : /join/i.test(pred[1]) ? 'JOIN' : /sarg/i.test(pred[1]) ? 'SARG' : /resid/i.test(pred[1]) ? 'RESID' : '';
      if (kind) payload.predicates.push({ operator_id: id, how_applied: kind, text: pred[2].trim() });
    }
  }
  const objects = text.split(/^Objects Used in Access Plan:\s*$/m)[1] ?? '';
  for (const obj of objects.split(/^\s*Schema:\s*/m).slice(1)) {
    const schema = obj.split('\n', 1)[0].trim();
    const name = obj.match(/^\s*Name:\s*([^\n]+)/m)?.[1].trim();
    const type = obj.match(/^\s*Type:\s*([^\n]+)/m)?.[1].trim();
    const tableSchema = obj.match(/^\s*Base Table Schema:\s*([^\n]+)/m)?.[1].trim();
    const tableName = obj.match(/^\s*Base Table Name:\s*([^\n]+)/m)?.[1].trim();
    if (type === 'Index' && name && tableSchema && tableName) payload.indexes.push({ schema, name, table_schema: tableSchema, table_name: tableName });
  }
  if (!payload.operators.length) throw new Error('Db2 report has no operator details');
  const graph = text.match(/^Access Plan:\s*$([\s\S]*?)(?=^Extended Diagnostic Information:|^Plan Details:)/m)?.[1] ?? '';
  const graphIds = [...graph.matchAll(/\(\s*(\d+)\s*\)/g)].map((match) => Number(match[1]));
  if (!graphIds.length || graphIds.some((id) => !payload.operators.some((operator) => operator.id === id))) throw new Error('Db2 report has incomplete operator details');
  const ids = new Set(payload.operators.map((op) => op.id));
  if (ids.size !== payload.operators.length || payload.streams.some((stream) => stream.source_type === 'O' && !ids.has(stream.source_id))) throw new Error('Db2 report has incomplete operator details');
  const consumed = new Set(payload.streams.filter((stream) => stream.source_type === 'O').map((stream) => stream.source_id));
  if (graphIds.length > 1 && graphIds.filter((id) => !consumed.has(id)).length !== 1) throw new Error('Db2 report has incomplete input streams');
  return payload;
}
