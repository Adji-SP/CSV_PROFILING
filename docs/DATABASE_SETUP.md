# SQLite now, Azure PostgreSQL when ready

You can use either database without replacing the CSV backend.
The Rust service stores OTA metadata, devices, deployments and TinyML reporting
records in SQL. Flask still handles the existing CSV processing separately.

This is **one active database per server**, not automatic synchronization or
dual-writing. Firmware binaries remain on disk, and recent console logs remain
in memory. A cloud database does not copy those files or keep the laptop awake.

## Local default: keep SQLite

In the ignored root .env:

```dotenv
DB_BACKEND=sqlite
```

Leave DATABASE_URL unset. Rust uses OTA_DATA_DIR/ota.db, normally data/ota.db.
If you intentionally want another file:

```dotenv
DB_BACKEND=sqlite
DATABASE_URL=sqlite:D:/my-data/ota.db?mode=rwc
```

Do not switch paths accidentally: an empty device list may mean you selected a
new database. Restart the backend after changing settings. Back up a SQLite
database using its backup API, or stop the server cleanly before making a file
backup; do not copy only a live .db and ignore its WAL.

## Azure: PostgreSQL Flexible Server

Use Azure Database for PostgreSQL, not Azure SQL Database (SQL Server), Cosmos DB,
or a database exposed through Cloudflare. Creating an Azure resource can cost money;
this project does not create one automatically.

Create an empty database and a dedicated application role with the schema rights
required by the migrations. Restrict network access to your backend/private
network and authorized development clients. Do not enable broad Internet access
just to get past a connection error.

Then set these privately in .env:

```dotenv
DB_BACKEND=postgres
DATABASE_URL=postgresql://ota_user:URL_ENCODED_PASSWORD@YOUR_SERVER.postgres.database.azure.com:5432/ota?sslmode=verify-full
```

Replace the placeholders. Percent-encode special characters in the username/password
so that @, #, /, ?, and similar characters are not parsed as URL delimiters.
Never paste the real URL into chat, logs, screenshots, or tracked files.

The configuration requires verify-full for Azure hostnames. Provide an up-to-date
trusted CA chain with sslrootcert if the default trust store is insufficient.
Do not change to sslmode=disable/require to suppress certificate validation errors.
Follow [Microsoft's TLS connection guidance](https://learn.microsoft.com/en-us/azure/postgresql/security/security-tls-how-to-connect)
for current certificate requirements.

Unknown/duplicate PostgreSQL URL options and DB_BACKEND/URL mismatches fail clearly
instead of silently choosing another database. With DB_BACKEND omitted, an existing
PostgreSQL DATABASE_URL still selects PostgreSQL for backward compatibility.

## What happens to existing data?

Nothing is automatically transferred. Selecting Azure points Rust at another
database. The existing SQLite file stays intact, but its devices/builds/reports
do not appear in the new database.

For a new development environment, let startup initialize the empty schema and
re-register test devices. To move real history, plan and verify a separate migration
including relationships, firmware files and stored paths. Do not blindly copy
SQLite SQL dumps into PostgreSQL.

The service applies the versioned OTA and TinyML migrations in ota-server/migrations/
and records them in schema_versions. Start one instance to apply migrations.
Use one full OTA/build server; shared build/filesystem management is not designed
for concurrent replicas. Give a separate monitoring-only instance a unique
MQTT_CLIENT_ID if it also consumes the broker.

Rollback means stopping the new instance and restoring the previous connection
settings and matching artifact storage/backup. It does not merge records created
in the other database.

## Tests you can repeat

Normal backend tests:

```powershell
cargo test --manifest-path .\ota-server\Cargo.toml
```

The two PostgreSQL tests are opt-in. On Windows, supply an installed/extracted
PostgreSQL bin directory:

```powershell
.\scripts\test-postgres.ps1 -PostgresBin 'C:\Program Files\PostgreSQL\17\bin'
```

This creates a fresh cluster under data/postgres-tests/<id>, binds it only to
127.0.0.1 on a temporary test port, uses a generated SCRAM password, runs both tests,
then stops that cluster. It leaves test data/logs for inspection and removes its
temporary password file. It does not install a service or use the real OTA database.

Alternatively TEST_DATABASE_URL may point to your own disposable PostgreSQL
database for cargo test postgres_ -- --ignored --test-threads=1.
Never use a production database for these tests.

SQLite tests and both PostgreSQL integration tests passed locally against
PostgreSQL 17.11. This verifies SQL compatibility, not an Azure deployment.
Azure networking, its live certificate chain and supplied credentials still
need validation when the resource exists.
