# Test fixtures

Program binaries the LiteSVM tests load besides the compiled `elote_pool`. Each one is pinned
by sha256 (`tests/common/mod.rs` for Core, `tests/squads.rs` for Squads) and checked at load, so a swapped binary fails the suite
instead of silently changing it.

| File | Program | Source | sha256 |
|---|---|---|---|
| `mpl_core_program-0.15.3.so` | Metaplex Core, `CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d` | GitHub release `release/core@0.15.3` (2026-10-07), asset `mpl_core_program.so`: https://github.com/metaplex-foundation/mpl-core/releases/download/release%2Fcore%400.15.3/mpl_core_program.so | `5b4dacdae4af476d3c9d8aa6712883f559ba0e13705f9b244d96d1fd0dd56ce5` |
| `squads_v4-mainnet.so` | Squads v4, `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf` | The mainnet deployment, dumped 2026-11-04 with `solana program dump -u m SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf` (upgrade authority none, last deployed at slot 302582236, 1,470,416 bytes). The devnet deployment is another build (`97fb2a5c…`) with the same compute in the spike. | `dec8d3e0fae58c7c8f2416e5f67c25e673f047afd6dd2bba4a47e0b29a01d34c` |

The program depends on the `mpl-core` Rust crate 0.12.1 for the CPI builders and account
decoders; the crate and the program binary are versioned separately. The LiteSVM round trips
(create through the crate's builders, read back through its decoders, against this binary) are
the compatibility proof: `tests/pool.rs` for collections and `tests/pagare.rs` for assets and
their plugins.

To refresh the binary: download the release asset, run `shasum -a 256`, update the file name,
`MPL_CORE_RELEASE`, `MPL_CORE_FIXTURE` and `MPL_CORE_SHA256` in `tests/common/mod.rs` and this
table in the same commit. Keep each file under 2 MB.

Squads is loaded only by `tests/squads.rs`, which runs the council's instructions through the real
`vault_transaction_execute`; the other tests sign as the vault directly with signature verification
off. To refresh it: dump the mainnet program again, check that its upgrade authority is still none,
and update `SQUADS_FIXTURE`, `SQUADS_SHA256` and this table in the same commit.
