// Draws one tranche of an approved line: the borrower receives `--amount` and the program mints the
// tranche's pagaré (a frozen Metaplex Core asset owned by the pool, in the pool's collection) and its
// record. The borrower signs alone, over the FIX in force, which the script prints with the USDC
// to be delivered before asking; the admin pays the two accounts' rent as the app's relayer would.
// The pagaré hash is derived on chain from the approved template and terms.
//
//   npx tsx scripts/draw.ts --cluster local|devnet --pool <address> --line-id <n>
//        --borrower-keypair <path> --amount <MXN> [--signature-method wallet|psc|efirma
//        --cert-fingerprint <64 hex> --cert-salt <64 hex>] [--yes]
//
// --amount            the tranche in MXN pesos, up to 2 decimals, at most what the line has left
// --signature-method  how the pagaré text was signed: wallet (the default), psc or efirma
// --cert-fingerprint  the signing certificate's fingerprint; with psc or efirma, and only with
// --cert-salt         the salt kept in the expediente: the draw records sha256(salt || fingerprint),
//                     never the fingerprint, which can be looked up to the signer's RFC
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
  certCommitment,
  getDrawInstructionAsync,
  usdcForMxn,
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
  fmtMxn,
  hash32,
  loadSigner,
  parseMxn,
  required,
  sendOrFail,
} from "./lib/cli.ts";
import { ataOf, loadLine, loadPool } from "./lib/pool.ts";

const METHODS = { wallet: 0, psc: 1, efirma: 2 } as const; // state::SIGNATURE_*

checkFlags([
  "cluster", "pool", "line-id", "borrower-keypair", "amount", "signature-method", "cert-fingerprint", "cert-salt",
  "yes",
]);
const c = cluster();
const poolAddress = address(required("pool"));
const lineIdRaw = required("line-id");
if (!/^\d+$/.test(lineIdRaw)) fail("--line-id must be a whole number");
const lineId = BigInt(lineIdRaw);
const amount = parseMxn("amount", required("amount"));
const methodName = arg("signature-method") ?? "wallet";
if (!(methodName in METHODS)) fail("--signature-method must be wallet, psc or efirma");
const signatureMethod = METHODS[methodName as keyof typeof METHODS];
if (signatureMethod === 0 && (arg("cert-fingerprint") !== undefined || arg("cert-salt") !== undefined)) fail("--cert-fingerprint and --cert-salt go with psc or efirma only");

const certFingerprint = signatureMethod === 0 ? new Uint8Array(32) : await certCommitment(hash32("cert-salt"), hash32("cert-fingerprint"));
const admin = await loadSigner("admin", process.env.ELOTE_ADMIN_KEYPAIR ?? ".demo-keys/admin.json");
const borrower = await loadSigner("borrower", required("borrower-keypair"));

const pool = await loadPool(c, poolAddress);
if (pool.fixRate === 0n) fail("no FIX is published on this pool: the council or the feeder posts it first");
if (pool.paused) fail("the pool is paused: requests, approvals and draws are blocked until the council resumes it");
const { line, data: l } = await loadLine(c, poolAddress, lineId);
if (l.borrower !== borrower.address) fail(`the borrower key (${borrower.address}) is not this line's borrower (${l.borrower})`);
if (l.status !== LineStatus.Approved && l.status !== LineStatus.Active) fail(`the line is ${LineStatus[l.status]}: only an Approved or Active line draws`);
if (amount > l.principal - l.drawn) fail(`the line has MXN ${fmtMxn(l.principal - l.drawn)} left to draw`);

const drawIndex = l.draws;
// The borrower confirms the FIX and the USDC shown here; a FIX published in between fails the draw
// (FixMismatch) and the script must be run again.
const usdc = usdcForMxn(amount, pool.fixRate);
console.log(`FIX ${pool.fixRate} (x10^4 MXN per USD) of ${pool.fixDate}: MXN ${fmtMxn(amount)} delivered as ${fmt(usdc)} USDC`);
await confirmCluster(c, "draw");
const borrowerAsset = await ataOf(borrower.address, pool.assetMint);
const signature = await sendOrFail(c, `draw MXN ${fmtMxn(amount)}, pagaré ${drawIndex}`, borrower, [
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
    `amount       MXN ${fmtMxn(pagare.amountMxn)} as ${fmt(pagare.usdcDelivered)} USDC; drawn through MXN ${fmtMxn(pagare.drawnThrough)}`,
    `due          ${new Date(Number(pagare.dueAt) * 1000).toISOString()}`,
    `line now     drawn MXN ${fmtMxn(after.drawn)} of ${fmtMxn(after.principal)}, interest MXN ${fmtMxn(after.interestDue)}, ${LineStatus[after.status]}`,
    `signature    ${signature}`,
    `explorer     ${explorer(c, signature)}`,
  ].join("\n")
);
console.log(`\n${DISCLAIMER}`);
