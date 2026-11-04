// Publishes Banxico's FIX on a pool: the feeder alone, or the council through Squads (the first
// FIX, moves beyond 5% and same-day corrections are the council's). The source is the FIX of the
// SIE of Banxico; every publication is checkable against the DOF through its FixPosted event.
//
//   npx tsx scripts/post-fix.ts --cluster local|devnet --pool <address> --fix <18.3740>
//        [--fix-date <yyyymmdd>] [--feeder-keypair <path>] [--council-keypairs <a.json,b.json>] [--yes]
//
// --fix             MXN per USD, up to four decimals
// --fix-date        the date Banxico determined it (default: today in Mexico City, by the chain clock)
// --feeder-keypair  the pool's fx_feeder signs; without it the council publishes
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { address } from "@solana/kit";
import { fetchPool, getPostFixInstructionAsync, mexicoCityDate, parseFix } from "../clients/js/src/index.ts";
import { DISCLAIMER, arg, chainNow, checkFlags, cluster, confirmCluster, fail, integer, loadSigner, required, sendOrFail } from "./lib/cli.ts";
import { councilExecute, loadCouncil } from "./lib/council.ts";
import { loadPool } from "./lib/pool.ts";

checkFlags(["cluster", "pool", "fix", "fix-date", "feeder-keypair", "council-keypairs", "yes"]);
const c = cluster();
const poolAddress = address(required("pool"));
let fixRate: bigint;
try {
  fixRate = parseFix(required("fix"));
} catch (e) {
  fail((e as Error).message);
}
const now = await chainNow(c);
const fixDate = integer("fix-date", mexicoCityDate(now), 20_000_101, 21_991_231)!;
await confirmCluster(c, "post_fix");
const pool = await loadPool(c, poolAddress);
const validUntil = now + 86_400n;
const label = `post_fix ${fixRate} (x10^4 MXN per USD) for ${fixDate}`;
if (arg("feeder-keypair")) {
  const feeder = await loadSigner("feeder", arg("feeder-keypair")!);
  if (pool.fxFeeder !== feeder.address) fail(`${feeder.address} is not this pool's fx_feeder (${pool.fxFeeder})`);
  await sendOrFail(c, label, feeder, [await getPostFixInstructionAsync({ poster: feeder, multisig: pool.multisig, pool: poolAddress, fixRate, fixDate, validUntil })]);
} else {
  const council = await loadCouncil(c, pool.multisig);
  await councilExecute(c, council, label, [
    await getPostFixInstructionAsync({ poster: council.signer, multisig: pool.multisig, pool: poolAddress, fixRate, fixDate, validUntil }),
  ]).catch((e) => fail((e as Error).message));
}
const after = (await fetchPool(c.rpc, poolAddress, { commitment: "confirmed" })).data;
console.log(`  FIX in force: ${after.fixRate} for ${after.fixDate}`);
console.log(`\n${DISCLAIMER}`);
