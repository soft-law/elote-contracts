# Test fixtures

Program binaries the LiteSVM tests load besides the compiled `elote_pool`. Each one is pinned
by sha256 in `tests/common/mod.rs` and checked at load, so a swapped binary fails the suite
instead of silently changing it.

| File | Program | Source | sha256 |
|---|---|---|---|
| `mpl_core_program-0.15.3.so` | Metaplex Core, `CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d` | GitHub release `release/core@0.15.3` (2026-10-07), asset `mpl_core_program.so`: https://github.com/metaplex-foundation/mpl-core/releases/download/release%2Fcore%400.15.3/mpl_core_program.so | `5b4dacdae4af476d3c9d8aa6712883f559ba0e13705f9b244d96d1fd0dd56ce5` |

The program depends on the `mpl-core` Rust crate 0.12.1 for the CPI builders and account
decoders; the crate and the program binary are versioned separately. The LiteSVM round trips
(create through the crate's builders, read back through its decoders, against this binary) are
the compatibility proof: `tests/pool.rs` for collections and `tests/pagare.rs` for assets and
their plugins.

To refresh the binary: download the release asset, run `shasum -a 256`, update the file name,
`MPL_CORE_RELEASE`, `MPL_CORE_FIXTURE` and `MPL_CORE_SHA256` in `tests/common/mod.rs` and this
table in the same commit. Keep the file under 1 MB.
