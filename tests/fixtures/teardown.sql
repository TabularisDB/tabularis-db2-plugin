-- =============================================================
-- Drops every fixture schema and the objects in it, so the
-- fixtures can be loaded again without recreating the database.
-- Run with: db2 -tf teardown.sql   (errors for objects that do
-- not exist are expected and ignored by the caller)
-- =============================================================

-- ADMIN_DROP_SCHEMA records what it could not drop (a schema that was not
-- there, for example) in the table named in its last two arguments, and
-- refuses to run when that table is left over from a previous call.
DROP TABLE TABULARIS_DROP_ERR.ERRORS;
DROP SCHEMA TABULARIS_DROP_ERR RESTRICT;

CALL SYSPROC.ADMIN_DROP_SCHEMA('TEST_SCHEMA', NULL, 'TABULARIS_DROP_ERR', 'ERRORS');
DROP TABLE TABULARIS_DROP_ERR.ERRORS;

CALL SYSPROC.ADMIN_DROP_SCHEMA('EMPTY_SCHEMA', NULL, 'TABULARIS_DROP_ERR', 'ERRORS');
DROP TABLE TABULARIS_DROP_ERR.ERRORS;

CALL SYSPROC.ADMIN_DROP_SCHEMA('Mixed Case', NULL, 'TABULARIS_DROP_ERR', 'ERRORS');
DROP TABLE TABULARIS_DROP_ERR.ERRORS;

DROP SCHEMA TABULARIS_DROP_ERR RESTRICT;
