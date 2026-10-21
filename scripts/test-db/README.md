# Test databases

Sample schema (customers/orders with a foreign key, a view, a procedure, a
function) for SQL Server, PostgreSQL and MySQL in Docker. Used by
`crates/sqlotus-db/tests/live.rs` and `scripts/e2e-db.mjs`.

```bash
export SQLOTUS_TEST_PASSWORD='Choose_a_Str0ng_one!'
docker run -d --name sqlotus-mssql -e ACCEPT_EULA=Y -e "MSSQL_SA_PASSWORD=$SQLOTUS_TEST_PASSWORD" -p 14333:1433 mcr.microsoft.com/mssql/server:2022-latest
docker run -d --name sqlotus-pg -e "POSTGRES_PASSWORD=$SQLOTUS_TEST_PASSWORD" -p 54329:5432 postgres:17
docker run -d --name sqlotus-mysql -e "MYSQL_ROOT_PASSWORD=$SQLOTUS_TEST_PASSWORD" -e MYSQL_DATABASE=shop -p 33069:3306 mysql:8.4
```

`ACCEPT_EULA=Y` accepts Microsoft's license terms for the SQL Server container image (the Developer edition it
runs by default is for development and testing only) — read them before you run the command. PostgreSQL and MySQL
images come under their own licenses.

Once the servers accept connections:

```bash
MSYS_NO_PATHCONV=1 docker cp "$(cygpath -w scripts/test-db/mssql.sql)" sqlotus-mssql:/tmp/seed.sql
MSYS_NO_PATHCONV=1 docker exec sqlotus-mssql /opt/mssql-tools18/bin/sqlcmd -S localhost -U sa -P "$SQLOTUS_TEST_PASSWORD" -C -b -i /tmp/seed.sql
docker exec -i sqlotus-pg psql -U postgres -v ON_ERROR_STOP=1 -q < scripts/test-db/pg.sql
docker exec -i sqlotus-mysql mysql -uroot -p"$SQLOTUS_TEST_PASSWORD" --default-character-set=utf8mb4 shop < scripts/test-db/mysql.sql
```

Live Rust tests:

```bash
export SQLOTUS_TEST_MSSQL="{\"dialect\":\"mssql\",\"host\":\"localhost,14333\",\"auth\":\"sql\",\"user\":\"sa\",\"password\":\"$SQLOTUS_TEST_PASSWORD\",\"database\":\"Shop\"}"
export SQLOTUS_TEST_PG="{\"dialect\":\"postgres\",\"host\":\"localhost\",\"port\":54329,\"user\":\"postgres\",\"password\":\"$SQLOTUS_TEST_PASSWORD\",\"database\":\"shop\"}"
export SQLOTUS_TEST_MYSQL="{\"dialect\":\"mysql\",\"host\":\"127.0.0.1\",\"port\":33069,\"user\":\"root\",\"password\":\"$SQLOTUS_TEST_PASSWORD\",\"database\":\"shop\"}"
cargo test -p sqlotus-db --test live
```

UI end-to-end (app running with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`):

```bash
SQLOTUS_E2E_PASSWORD="$SQLOTUS_TEST_PASSWORD" node scripts/e2e-db.mjs
```
