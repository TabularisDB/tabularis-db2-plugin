-- =============================================================
-- Stored procedures and functions for integration tests
-- Run with: db2 -td@ -vf routines.sql
-- (uses @ as statement terminator)
-- =============================================================

CREATE PROCEDURE TEST_SCHEMA.GET_EMPLOYEES_BY_DEPT(
    IN p_dept_id INTEGER
)
LANGUAGE SQL
DYNAMIC RESULT SETS 1
BEGIN
    DECLARE c1 CURSOR WITH RETURN FOR
        SELECT EMP_ID, FIRST_NAME, LAST_NAME, EMAIL, SALARY
        FROM TEST_SCHEMA.EMPLOYEES
        WHERE DEPT_ID = p_dept_id
        ORDER BY LAST_NAME;
    OPEN c1;
END @

CREATE PROCEDURE TEST_SCHEMA.UPDATE_SALARY(
    IN p_emp_id INTEGER,
    IN p_new_salary DECIMAL(10, 2),
    OUT p_old_salary DECIMAL(10, 2)
)
LANGUAGE SQL
BEGIN
    SELECT SALARY INTO p_old_salary
    FROM TEST_SCHEMA.EMPLOYEES
    WHERE EMP_ID = p_emp_id;

    UPDATE TEST_SCHEMA.EMPLOYEES
    SET SALARY = p_new_salary
    WHERE EMP_ID = p_emp_id;
END @

CREATE FUNCTION TEST_SCHEMA.FULL_NAME(
    p_first_name VARCHAR(50),
    p_last_name VARCHAR(50)
)
RETURNS VARCHAR(101)
LANGUAGE SQL
DETERMINISTIC
NO EXTERNAL ACTION
CONTAINS SQL
BEGIN ATOMIC
    RETURN p_first_name || ' ' || p_last_name;
END @

CREATE FUNCTION TEST_SCHEMA.DEPT_EMPLOYEE_COUNT(
    p_dept_id INTEGER
)
RETURNS INTEGER
LANGUAGE SQL
NOT DETERMINISTIC
READS SQL DATA
BEGIN ATOMIC
    RETURN (SELECT COUNT(*) FROM TEST_SCHEMA.EMPLOYEES WHERE DEPT_ID = p_dept_id);
END @

-- Two functions of one name: Db2 tells them apart by their arguments.
CREATE FUNCTION TEST_SCHEMA.FORMAT_AMOUNT(
    p_amount DECIMAL(10, 2)
)
RETURNS VARCHAR(20)
LANGUAGE SQL
DETERMINISTIC
NO EXTERNAL ACTION
CONTAINS SQL
RETURN TRIM(CHAR(p_amount)) @

CREATE FUNCTION TEST_SCHEMA.FORMAT_AMOUNT(
    p_amount DECIMAL(10, 2),
    p_currency CHAR(3)
)
RETURNS VARCHAR(24)
LANGUAGE SQL
DETERMINISTIC
NO EXTERNAL ACTION
CONTAINS SQL
RETURN TRIM(CHAR(p_amount)) || ' ' || p_currency @

-- A function without arguments: its signature is empty, not missing.
CREATE FUNCTION TEST_SCHEMA.FIXTURE_VERSION()
RETURNS VARCHAR(10)
LANGUAGE SQL
DETERMINISTIC
NO EXTERNAL ACTION
CONTAINS SQL
RETURN '1.0' @

-- A table function: what it returns is a set of columns, not one value.
CREATE FUNCTION TEST_SCHEMA.EMPLOYEES_OF_DEPT(
    p_dept_id INTEGER
)
RETURNS TABLE (EMP_ID INTEGER, LAST_NAME VARCHAR(50))
LANGUAGE SQL
READS SQL DATA
RETURN
    SELECT EMP_ID, LAST_NAME
    FROM TEST_SCHEMA.EMPLOYEES
    WHERE DEPT_ID = p_dept_id @

-- A procedure with a parameter of every direction.
CREATE PROCEDURE TEST_SCHEMA.ADJUST_BUDGET(
    IN p_dept_id INTEGER,
    OUT p_previous DECIMAL(15, 2),
    INOUT p_amount DECIMAL(15, 2)
)
LANGUAGE SQL
BEGIN
    SELECT BUDGET INTO p_previous
    FROM TEST_SCHEMA.DEPARTMENTS
    WHERE DEPT_ID = p_dept_id;

    SET p_amount = COALESCE(p_previous, 0) + p_amount;
END @
