# DuckDB Athena Extension

> **WARNING** This is a work in progress - things may or may not work as expected 🧙‍♂️

Query Amazon Athena tables directly from DuckDB using `athena_scan`.

## Limitations

- Only the `default` Glue database is supported
- Not all data types are implemented yet
- 10,000 results are returned by default (use `maxrows=-1` to return everything)
- Filter pushdown is not supported — the full table is always scanned

## Prerequisites

- [Rust](https://rustup.rs/) stable toolchain (`rustup install stable`)
- [DuckDB CLI](https://duckdb.org/docs/installation/) v1.4 or later
- AWS credentials with permissions for Athena, Glue, and S3

## Building from source

Clone the repository and build with Cargo:

```bash
git clone https://github.com/ebunt/duckdb-athena-extension.git
cd duckdb-athena-extension
cargo build --release
```

The compiled extension is placed in `target/release/` with a platform-specific name:

| Platform | File |
|---|---|
| Linux | `target/release/libduckdb_athena.so` |
| macOS | `target/release/libduckdb_athena.dylib` |
| Windows | `target/release/duckdb_athena.dll` |

## AWS credentials

The extension reads AWS credentials and region from the standard environment variables:

```bash
export AWS_ACCESS_KEY_ID=<your-access-key>
export AWS_SECRET_ACCESS_KEY=<your-secret-key>
export AWS_REGION=us-east-1
```

Any credential source supported by the AWS SDK (instance profile, SSO, `~/.aws/credentials`, etc.) also works.

The IAM principal needs at minimum:
- `athena:StartQueryExecution`, `athena:GetQueryExecution`, `athena:GetQueryResults`
- `glue:GetTable` on the target table
- `s3:PutObject` / `s3:GetObject` on the S3 results bucket

## Loading the extension

DuckDB must be started with the `-unsigned` flag to load locally built extensions:

```bash
AWS_REGION=us-east-1 duckdb -unsigned
```

Then load the extension file (adjust the path and filename for your platform):

```sql
-- Linux
LOAD 'target/release/libduckdb_athena.so';

-- macOS
LOAD 'target/release/libduckdb_athena.dylib';

-- Windows
LOAD 'target/release/duckdb_athena.dll';
```

## Usage

### Basic query

Provide the Glue table name and an S3 path where Athena should write query results:

```sql
SELECT * FROM athena_scan('my_table', 's3://my-results-bucket/prefix/');
```

### Return all rows

By default only the first 10,000 rows are returned. Pass `maxrows=-1` to remove the limit:

```sql
SELECT * FROM athena_scan('my_table', 's3://my-results-bucket/prefix/', maxrows=-1);
```

### Filter after scanning

Filter pushdown is not yet supported, so add WHERE clauses in DuckDB after the scan:

```sql
SELECT * FROM athena_scan('my_table', 's3://my-results-bucket/prefix/', maxrows=-1)
WHERE year = 2024;
```

## Testing

There are no automated unit tests in this repository yet. To verify the extension works end-to-end:

1. Build the extension as described above.
2. Export your AWS credentials and region.
3. Start DuckDB with `-unsigned` and load the extension.
4. Run a query against a known table in your default Glue catalog:

```sql
SELECT COUNT(*) FROM athena_scan('my_table', 's3://my-results-bucket/prefix/');
```

You should see console output like:

```
Running Athena query, execution id: 152a20c7-ff32-4a19-bb71-ae0135373ca6
State: Running, sleeping 5 secs...
Total execution time: 1307 millis
```

followed by the result set.

## Credits

- Rust DuckDB extension FFI: https://github.com/ywilkof/quack-rs
