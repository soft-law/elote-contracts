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
// post_fix takes RedStone's MXNe price account as the market reference. The feeder's FIX must be
// within 2% of it and the reference at most 26 hours old; the council publishes without that check.
// The account exists on mainnet only: a localnet or devnet build skips the check when it is missing.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { address } from "@solana/kit";
import {
  FX_REFERENCE_ACCOUNT,
  FX_REFERENCE_PROGRAM,
  MAX_FIX_REFERENCE_GAP_BPS,
  MAX_REFERENCE_AGE_SECONDS,
  fetchPool,
  getPostFixInstructionAsync,
  mexicoCityDate,
  parseFix,
  referenceRate,
  withinBps,
} from "../clients/js/src/index.ts";
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
// The reference the program reads, shown before the publication; the program decides.
const referenceAccount = await c.rpc.getAccountInfo(FX_REFERENCE_ACCOUNT, { encoding: "base64" }).send();
if (!referenceAccount.value) {
  console.log(`  FX reference: ${FX_REFERENCE_ACCOUNT} does not exist on this cluster (the check is skipped on localnet and devnet builds)`);
} else if (referenceAccount.value.owner !== FX_REFERENCE_PROGRAM) {
  console.log(`  FX reference: ${FX_REFERENCE_ACCOUNT} is not owned by RedStone's adapter; the feeder's publication will fail`);
} else {
  try {
    const { rate, at } = referenceRate(new Uint8Array(Buffer.from(referenceAccount.value.data[0], "base64")));
    const age = now - at;
    console.log(`  FX reference: ${rate} (x10^4 MXN per USD), ${age} s old`);
    if (arg("feeder-keypair") && age > MAX_REFERENCE_AGE_SECONDS) console.log("  the reference is older than 26 h: the feeder's publication will fail, the council can publish");
    if (arg("feeder-keypair") && !withinBps(rate, fixRate, MAX_FIX_REFERENCE_GAP_BPS))
      console.log("  the FIX is more than 2% from the reference: the feeder's publication will fail, the council can publish");
  } catch (e) {
    console.log(`  FX reference: ${(e as Error).message}`);
  }
}
const label = `post_fix ${fixRate} (x10^4 MXN per USD) for ${fixDate}`;
if (arg("feeder-keypair")) {
  const feeder = await loadSigner("feeder", arg("feeder-keypair")!);
  if (pool.fxFeeder !== feeder.address) fail(`${feeder.address} is not this pool's fx_feeder (${pool.fxFeeder})`);
  await sendOrFail(c, label, feeder, [await getPostFixInstructionAsync({ poster: feeder, multisig: pool.multisig, pool: poolAddress, fxReference: FX_REFERENCE_ACCOUNT, fixRate, fixDate, validUntil })]);
} else {
  const council = await loadCouncil(c, pool.multisig);
  await councilExecute(c, council, label, [
    await getPostFixInstructionAsync({
      poster: council.signer,
      multisig: pool.multisig,
      pool: poolAddress,
      fxReference: FX_REFERENCE_ACCOUNT,
      fixRate,
      fixDate,
      validUntil,
    }),
  ]).catch((e) => fail((e as Error).message));
}
const after = (await fetchPool(c.rpc, poolAddress, { commitment: "confirmed" })).data;
console.log(`  FIX in force: ${after.fixRate} for ${after.fixDate}`);
console.log(`\n${DISCLAIMER}`);
