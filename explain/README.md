# Db2 Visual EXPLAIN parser

Turns the payload the Db2 plugin's `explain_query` returns
(`engine: db2`, `format: db2-explain-tables-json`) into the plan tree
Tabularis's Visual EXPLAIN draws.

The payload carries the rows Db2's `EXPLAIN PLAN` wrote into its explain
tables for one statement (`EXPLAIN_OPERATOR`, `EXPLAIN_STREAM`,
`EXPLAIN_PREDICATE`, `EXPLAIN_ARGUMENT`, plus the tables of the indexes the
plan reads) and, with Analyze, the row count and elapsed time of the run.
`src/payload.ts` describes it; the plugin's `src/models.rs` produces it.

## npm usage

```bash
pnpm add @tabularis/explain @tabularis/explain-db2
```

Import `@tabularis/explain-db2` to register the parser with
`@tabularis/explain` (version 0.2.0 or newer), or import `parseDb2Explain`
directly to parse a payload string.

```ts
import "@tabularis/explain-db2";
import { parseRawExplain } from "@tabularis/explain";

const plan = parseRawExplain({
  engine: "db2",
  format: "db2-explain-tables-json",
  payload,
  original_query: "SELECT * FROM orders",
});
```

The release workflow publishes the package using npm Trusted Publishing.
Configure the trusted publisher on npm for `TabularisDB`, repository
`tabularis-db2-plugin`, and workflow `release.yml` before the first release.

The parser:

- makes tables and indexes the `relation` of the operator that reads them
- places predicates where Db2 applies them: index ranges in
  `index_condition`, filters in `filter`, the condition of a hash join in
  `hash_condition`
- reports join types from `OUTERJN`, and lists a join's outer input first
- keeps every operator argument in `extra`
- reads the compile time from `PREPTIME`

## Build and test

```bash
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test   # builds dist/index.iife.js, then runs vitest
```

`dist/index.iife.js` is the bundle `.tabularium` declares under
`explain_parsers`; the release workflow ships it in every plugin archive.

The fixtures in `tests/fixtures/` are payloads captured from Db2 12.1; the
plans they produce are committed in `tests/fixtures/expected/`. After an
intended change, regenerate them with `UPDATE_GOLDENS=1 pnpm test` and review
the diff.
