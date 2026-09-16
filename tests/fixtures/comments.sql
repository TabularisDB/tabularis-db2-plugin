-- Use dynamic SQL so the Db2 CLP does not normalize the newline inside
-- each COMMENT string literal to a space while reading this fixture.
CREATE PROCEDURE TEST_SCHEMA.SET_TEST_COMMENTS()
LANGUAGE SQL
BEGIN
    EXECUTE IMMEDIATE 'COMMENT ON TABLE TEST_SCHEMA.EMPLOYEES IS ''Employee''''s résumé' || CHR(10) || '所有员工''';
    EXECUTE IMMEDIATE 'COMMENT ON COLUMN TEST_SCHEMA.EMPLOYEES.NOTES IS ''Manager''''s notes — café' || CHR(10) || '第二行''';
END@

CALL TEST_SCHEMA.SET_TEST_COMMENTS()@
DROP PROCEDURE TEST_SCHEMA.SET_TEST_COMMENTS@
