// One council decision on a pool's settings, through Squads: proposed, approved by the threshold
// of members whose keys are given, and executed. Every decision expires a day after it is built.
//
//   npx tsx scripts/council-propose.ts --cluster local|devnet --pool <address> --op <op> [value flags]
//        [--council-keypairs <a.json,b.json>] [--yes]
//
//   --op set-fx-feeder --address <key>       the key that publishes the FIX (11111111111111111111111111111111 for none)
//   --op set-max-line-bps --bps <1..10000>   the largest line as a share of the NAV
//   --op set-outstanding-cap --usdc <amount> the most USDC the pool lends at once
//   --op set-fix-bounds --min <18.0000> --max <22.0000>
//                                            the lowest and highest FIX post_fix accepts (0 and 0 for none)
//   --op reject-line --line-id <n>           close a requested line; its rent returns to the payer
//   --op resume                              resume a paused pool
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { type Instruction, address } from "@solana/kit";
import {
  getRejectLineInstructionAsync,
  getResumeInstructionAsync,
  getSetFixBoundsInstructionAsync,
  getSetFxFeederInstructionAsync,
  getSetMaxLineBpsInstructionAsync,
  getSetOutstandingCapInstructionAsync,
  parseFix,
} from "../clients/js/src/index.ts";
import { DISCLAIMER, arg, chainNow, checkFlags, cluster, confirmCluster, fail, integer, parseAmount, required } from "./lib/cli.ts";
import { councilExecute, loadCouncil } from "./lib/council.ts";
import { loadLine, loadPool } from "./lib/pool.ts";

checkFlags(["cluster", "pool", "op", "address", "bps", "usdc", "min", "max", "line-id", "council-keypairs", "yes"]);
const c = cluster();
const poolAddress = address(required("pool"));
const op = required("op");
await confirmCluster(c, op);
const pool = await loadPool(c, poolAddress);
const council = await loadCouncil(c, pool.multisig);
const common = { council: council.signer, multisig: pool.multisig, pool: poolAddress, validUntil: (await chainNow(c)) + 86_400n };

let ix: Instruction;
switch (op) {
  case "set-fx-feeder":
    ix = await getSetFxFeederInstructionAsync({ ...common, fxFeeder: address(required("address")) });
    break;
  case "set-max-line-bps":
    ix = await getSetMaxLineBpsInstructionAsync({ ...common, maxLineBps: integer("bps", undefined, 1, 10_000) ?? fail("--bps is required") });
    break;
  case "set-outstanding-cap":
    ix = await getSetOutstandingCapInstructionAsync({ ...common, maxOutstandingPrincipal: parseAmount("usdc", required("usdc")) });
    break;
  case "set-fix-bounds": {
    // "0" is no bound; anything else is a FIX with at most four decimals.
    const fix = (flag: string) => {
      const raw = required(flag);
      try {
        return raw === "0" ? 0n : parseFix(raw);
      } catch (e) {
        return fail((e as Error).message);
      }
    };
    ix = await getSetFixBoundsInstructionAsync({ ...common, minFixRate: fix("min"), maxFixRate: fix("max") });
    break;
  }
  case "reject-line": {
    const raw = required("line-id");
    if (!/^\d+$/.test(raw)) fail("--line-id must be a whole number");
    const { line, data } = await loadLine(c, poolAddress, BigInt(raw));
    ix = await getRejectLineInstructionAsync({ ...common, line, payer: data.payer });
    break;
  }
  case "resume":
    ix = await getResumeInstructionAsync(common);
    break;
  default:
    fail(`unknown --op ${op}`);
}
if (arg("address") !== undefined && !op.endsWith("feeder")) fail("--address goes with set-fx-feeder");
if ((arg("min") !== undefined || arg("max") !== undefined) && op !== "set-fix-bounds") fail("--min and --max go with set-fix-bounds");
await councilExecute(c, council, op, [ix]).catch((e) => fail((e as Error).message));
console.log(`\n${DISCLAIMER}`);
