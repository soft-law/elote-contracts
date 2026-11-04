// The council approves a requested line through Squads. The approval names the expediente root and
// the terms hash, which the curators compute from the documents; this script recomputes the hash
// from the terms the line stores (clients/js termsHash) and refuses if it is not the stored one.
// The decision expires a day after it is built (valid_until).
//
//   npx tsx scripts/approve-line.ts --cluster local|devnet --pool <address> --line-id <n>
//        [--council-keypairs <a.json,b.json>] [--yes]
//
// The council members' keys come from --council-keypairs, ELOTE_COUNCIL_KEYPAIRS or
// .demo-keys/curator-<n>.json; ELOTE_RPC overrides the cluster's RPC URL.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { address } from "@solana/kit";
import { LineStatus } from "../clients/js/src/index.ts";
import { DISCLAIMER, checkFlags, cluster, confirmCluster, fail, fmtMxn, required } from "./lib/cli.ts";
import { approveLine, loadCouncil } from "./lib/council.ts";
import { loadLine, loadPool } from "./lib/pool.ts";

checkFlags(["cluster", "pool", "line-id", "council-keypairs", "yes"]);
const c = cluster();
const poolAddress = address(required("pool"));
const lineIdRaw = required("line-id");
if (!/^\d+$/.test(lineIdRaw)) fail("--line-id must be a whole number");
const lineId = BigInt(lineIdRaw);
await confirmCluster(c, "approve_line");
const pool = await loadPool(c, poolAddress);
const council = await loadCouncil(c, pool.multisig);
await approveLine(c, council, poolAddress, lineId).catch((e) => fail((e as Error).message));
const after = (await loadLine(c, poolAddress, lineId)).data;
console.log(
  `  status       ${LineStatus[after.status]}\n  principal    MXN ${fmtMxn(after.principal)}\n  draws until  ${new Date(Number(after.availabilityEnd) * 1000).toISOString()}`
);
console.log(`\n${DISCLAIMER}`);
