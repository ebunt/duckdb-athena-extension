# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build

Always use `make` rather than `cargo build --release` directly — the Makefile handles both compilation and the required `.duckdb_extension` rename step:

```bash
make        # build release + copy to target/release/duckdb_athena.duckdb_extension
make clean  # cargo clean
```

DuckDB 1.0+ only loads files ending in `.duckdb_extension`. The Makefile automates the copy from the platform-specific output (`libduckdb_athena.dylib` on macOS, `.so` on Linux, `.dll` on Windows).

## Loading the extension in DuckDB

```bash
AWS_REGION=us-east-1 duckdb -unsigned -cmd "SET allow_extensions_metadata_mismatch=true;"
```

```sql
LOAD 'target/release/duckdb_athena.duckdb_extension';
```

The `-unsigned` flag and `allow_extensions_metadata_mismatch` setting are required because locally compiled extensions lack the build metadata DuckDB expects from official releases.

## Testing

There are no automated tests. End-to-end verification requires live AWS credentials:

```bash
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
export AWS_REGION=us-east-1
```

Required IAM permissions: `athena:StartQueryExecution`, `athena:GetQueryExecution`, `athena:GetQueryResults`, `glue:GetTable`, `s3:PutObject`/`s3:GetObject` on the results bucket.

## Architecture

This is a Rust `cdylib` crate that implements a DuckDB loadable extension exposing a single table function, `athena_scan`.

**Entry point** (`src/lib.rs`): Uses the `quack-rs` `entry_point!` macro to register `athena_scan` with DuckDB on load. A `lazy_static` Tokio runtime is created once and reused across all calls (blocking async AWS SDK calls with `RUNTIME.block_on(...)`).

**Table function lifecycle** (`src/table_function.rs`): DuckDB's table function API has three phases:
1. **Bind** (`read_athena_bind`): Called once at query planning time. Reads named/positional parameters, calls Glue `GetTable` to discover the schema, and registers result columns with DuckDB. Stores connection params in `ScanBindData`.
2. **Init** (`read_athena_init`): Called once before scanning. Submits the Athena query (`SELECT * FROM "db"."table" LIMIT N`), polls `GetQueryExecution` every 5 seconds until done, then paginates all `GetQueryResults` pages into `ScanInitData`.
3. **Scan** (`read_athena`): Called repeatedly by DuckDB to consume data. Serves one page per call from the pre-fetched `pages` vec; signals completion by setting chunk size to 0.

**Type mapping** (`src/types.rs`): `map_type` converts Athena/Glue type strings to DuckDB `TypeId`s. `populate_column` writes parsed values into DuckDB vectors. Types not in the mapping fall back to `Varchar`. Unsupported complex types (arrays, maps, structs) are not yet implemented.

**Key constraints**:
- `maxrows` defaults to 10,000; pass `maxrows=-1` to remove the limit (translates to no `LIMIT` clause in the Athena SQL)
- `database` defaults to `"default"` (the Glue database name)
- Filter pushdown is not implemented — all filtering happens in DuckDB after the full scan
- The first row of Athena's first result page is always the column header and is skipped in `read_athena`
