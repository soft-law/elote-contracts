// Draws one tranche of an approved line: the borrower receives `--amount` and the program mints the
// tranche's pagaré (a frozen Metaplex Core asset owned by the pool, in the pool's collection) and its
// record. The borrower signs alone, over the FIX in force, which the script prints with the USDC
// to be delivered before asking; the admin pays the two accounts' rent as the app's relayer would.
// The pagaré hash is derived on chain from the approved template and terms.
//
//   npx tsx scripts/draw.ts --cluster local|devnet --pool <address> --line-id <n>
//        --borrower-keypair <path> --amount <MXN> [--signature-method wallet|psc|efirma
//        --cert-fingerprint <64 hex>] [--yes]
//
// --amount            the tranche in the line's units, at most what the line has left
// --signature-method  how the pagaré text was signed: wallet (the default), psc or efirma
// --cert-fingerprint  hash of the signing certificate; required with psc or efirma
// --yes               send without asking when the cluster is not this machine
//
// The admin key comes from ELOTE_ADMIN_KEYPAIR or .demo-keys/admin.json; ELOTE_RPC overrides the
// cluster's RPC URL. Keys are never printed, only addresses.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { address } from "@solana/kit";
import { SYSTEM_PROGRAM_ADDRESS } from "@solana-program/system";
import { TOKEN_PROGRAM_ADDRESS, getCreateAssociatedTokenIdempotentInstructionAsync } from "@solana-program/token";
import {
  LineStatus,
  drawPagareAccounts,
  fetchCreditLine,
  fetchPagare,
  findPagarePda,
  getDrawInstructionAsync,
} from "../clients/js/src/index.ts";
import {
  DISCLAIMER,
  arg,
  checkFlags,
  cluster,
  confirmCluster,
  explorer,
  fail,
  fmt,
  hash32,
  loadSigner,
  parseAmount,
  required,
  sendOrFail,
} from "./lib/cli.ts";
import { ataOf, loadLine, loadPool } from "./lib/pool.ts";

const METHODS = { wallet: 0, psc: 1, efirma: 2 } as const; // state::SIGNATURE_*

checkFlags([
  "cluster", "pool", "line-id", "borrower-keypair", "amount", "signature-method", "cert-fingerprint", "yes",
]);
const c = cluster();
const poolAddress = address(required("pool"));
const lineIdRaw = required("line-id");
if (!/^\d+$/.test(lineIdRaw)) fail("--line-id must be a whole number");
const lineId = BigInt(lineIdRaw);
const amount = parseAmount("amount", required("amount"));
const methodName = arg("signature-method") ?? "wallet";
if (!(methodName in METHODS)) fail("--signature-method must be wallet, psc or efirma");
const signatureMethod = METHODS[methodName as keyof typeof METHODS];
if (signatureMethod === 0 && arg("cert-fingerprint") !== undefined) fail("--cert-fingerprint goes with psc or efirma only");

const certFingerprint = signatureMethod === 0 ? new Uint8Array(32) : hash32("cert-fingerprint");
const admin = await loadSigner("admin", process.env.ELOTE_ADMIN_KEYPAIR ?? ".demo-keys/admin.json");
const borrower = await loadSigner("borrower", required("borrower-keypair"));

const pool = await loadPool(c, poolAddress);
if (pool.fixRate === 0n) fail("no FIX is published on this pool: the council or the feeder posts it first");
if (pool.paused) fail("the pool is paused: openings and draws are blocked until the admin resumes it");
const { line, data: l } = await loadLine(c, poolAddress, lineId);
if (l.borrower !== borrower.address) fail(`the borrower key (${borrower.address}) is not this line's borrower (${l.borrower})`);
if (l.status !== LineStatus.Approved && l.status !== LineStatus.Active) fail(`the line is ${LineStatus[l.status]}: only an Approved or Active line draws`);
if (amount > l.principal - l.drawn) fail(`the line has ${fmt(l.principal - l.drawn)} tMXN left to draw`);

const drawIndex = l.draws;
// The borrower confirms the FIX and the USDC shown here; a FIX published in between fails the draw
// (FixMismatch) and the script must be run again.
const usdc = (amount * 100_000_000n) / pool.fixRate;
console.log(`FIX ${pool.fixRate} (x10^4 MXN per USD) of ${pool.fixDate}: ${fmt(amount)} MXN delivered as ${usdc} USDC base units`);
await confirmCluster(c, "draw");
const borrowerAsset = await ataOf(borrower.address, pool.assetMint);
const signature = await sendOrFail(c, `draw ${fmt(amount)} tMXN, pagaré ${drawIndex}`, borrower, [
  await getCreateAssociatedTokenIdempotentInstructionAsync({ payer: borrower, owner: borrower.address, mint: pool.assetMint }),
  await getDrawInstructionAsync({
    borrower,
    pool: poolAddress,
    line,
    assetMint: pool.assetMint,
    vault: pool.vault,
    borrowerAssetAccount: borrowerAsset,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
    payer: admin,
    ...(await drawPagareAccounts({ line, drawIndex, collection: pool.collection })),
    systemProgram: SYSTEM_PROGRAM_ADDRESS,
    amountMxn: amount,
    fxRate: pool.fixRate,
    fxDate: pool.fixDate,
    certFingerprint,
    signatureMethod,
  }),
]);

const after = (await fetchCreditLine(c.rpc, line, { commitment: "confirmed" })).data;
const [record] = await findPagarePda({ line, drawIndex });
const pagare = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
console.log(
  [
    `line         ${line} (id ${lineId})`,
    `pagaré       ${drawIndex}: asset ${pagare.asset}, record ${record}`,
    `amount       ${fmt(pagare.amountMxn)} tMXN; drawn through ${fmt(pagare.drawnThrough)}`,
    `due          ${new Date(Number(pagare.dueAt) * 1000).toISOString()}`,
    `line now     drawn ${fmt(after.drawn)} of ${fmt(after.principal)}, interest ${fmt(after.interestDue)}, ${LineStatus[after.status]}`,
    `signature    ${signature}`,
    `explorer     ${explorer(c, signature)}`,
  ].join("\n")
);
console.log(`\n${DISCLAIMER}`);
