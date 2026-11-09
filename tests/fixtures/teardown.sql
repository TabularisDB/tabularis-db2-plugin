-- =============================================================
-- Drops every fixture schema and the objects in it, so the
-- fixtures can be loaded again without recreating the database.
-- Run with: db2 -tf teardown.sql   (errors for objects that do
-- not exist are expected and ignored by the caller)
-- =============================================================

-- A compound statement run as dynamic SQL (BEGIN ... END from the editor or
-- a test) is compiled into a package in the schema that was current. The
-- package lives while the statement is in the package cache, and stays
-- behind as inoperative when a table it used is dropped. A schema that
-- still holds one cannot be dropped, so the cache is emptied and whatever
-- packages remain are dropped, one failure not stopping the others.
FLUSH PACKAGE CACHE DYNAMIC;

--#SET TERMINATOR @
BEGIN
    DECLARE CONTINUE HANDLER FOR SQLEXCEPTION BEGIN END;
    FOR PKG AS
        SELECT PKGSCHEMA, PKGNAME FROM SYSCAT.PACKAGES
        WHERE PKGSCHEMA IN ('TEST_SCHEMA', 'EMPTY_SCHEMA', 'Mixed Case')
    DO
        EXECUTE IMMEDIATE
            'DROP PACKAGE "' || PKG.PKGSCHEMA || '"."' || PKG.PKGNAME || '"';
    END FOR;
END@
--#SET TERMINATOR ;

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
