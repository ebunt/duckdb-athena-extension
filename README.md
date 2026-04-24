# DuckDB Athena Extension

> **WARNING** This is a work in progress - things may or may not work as expected 🧙‍♂️

Query Amazon Athena tables directly from DuckDB using `athena_scan`.

## Limitations

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

| Platform | Raw output file |
|---|---|
| Linux | `target/release/libduckdb_athena.so` |
| macOS | `target/release/libduckdb_athena.dylib` |
| Windows | `target/release/duckdb_athena.dll` |

DuckDB 1.0+ only loads files with the `.duckdb_extension` suffix, so copy the output to the required name:

```bash
# Linux
cp target/release/libduckdb_athena.so target/release/duckdb_athena.duckdb_extension

# macOS
cp target/release/libduckdb_athena.dylib target/release/duckdb_athena.duckdb_extension

# Windows (PowerShell)
Copy-Item target\release\duckdb_athena.dll target\release\duckdb_athena.duckdb_extension
```

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

DuckDB 1.0+ requires two things when loading a locally compiled extension:

1. **`.duckdb_extension` suffix** – DuckDB will refuse to load any file that does not end with `.duckdb_extension`.
2. **Metadata-mismatch bypass** – locally compiled extensions do not carry the same build metadata that DuckDB expects from official releases, so the `allow_extensions_metadata_mismatch` setting must be enabled.

Start DuckDB with the `-unsigned` flag and set `allow_extensions_metadata_mismatch` before loading:

```bash
AWS_REGION=us-east-1 duckdb -unsigned -cmd "SET allow_extensions_metadata_mismatch=true;"
```

Then load the extension (the path is the same on all platforms after the copy step above):

```sql
LOAD 'target/release/duckdb_athena.duckdb_extension';
```

## Usage

### Basic query

Provide the Glue table name and an S3 path where Athena should write query results:

```sql
SELECT * FROM athena_scan('my_table', 's3://my-results-bucket/prefix/');
```

By default the `default` Glue database is used. To query a table in a different database pass the `database` named parameter:

```sql
SELECT * FROM athena_scan('my_table', 's3://my-results-bucket/prefix/', database='my_database');
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
3. Start DuckDB with `-unsigned` and `allow_extensions_metadata_mismatch=true`, then load the extension.
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
