// Draws one tranche of an approved line: the borrower receives `--amount` and the program mints the
// tranche's pagaré (a frozen Metaplex Core asset owned by the pool, in the pool's collection) and its
// record. The borrower and the pool's attestor sign; the admin pays the two accounts' rent as the
// app's relayer would.
//
//   npx tsx scripts/draw.ts --cluster local|devnet --pool <address> --line-id <n>
//        --borrower-keypair <path> --amount <tMXN> [--document-hash <64 hex>]
//        [--signature-method wallet|psc|efirma --cert-fingerprint <64 hex>]
//        [--attestor-keypair <path>] [--yes]
//
// --amount            the tranche in tMXN, up to 6 decimals; at most what the line has left
// --document-hash     hash of the signed pagaré text (a placeholder if absent)
// --signature-method  how the pagaré text was signed: wallet (the default), psc or efirma
// --cert-fingerprint  hash of the signing certificate; required with psc or efirma
// --attestor-keypair  default ELOTE_ATTESTOR_KEYPAIR or .demo-keys/attestor.json
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
  roleSigner,
  sendOrFail,
} from "./lib/cli.ts";
import { ataOf, loadLine, loadPool } from "./lib/pool.ts";

const METHODS = { wallet: 0, psc: 1, efirma: 2 } as const; // state::SIGNATURE_*

checkFlags([
  "cluster", "pool", "line-id", "borrower-keypair", "amount", "document-hash", "signature-method",
  "cert-fingerprint", "attestor-keypair", "yes",
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

await confirmCluster(c, "draw");
const documentHash = hash32("document-hash", "elote.fun demo pagaré");
const certFingerprint = signatureMethod === 0 ? new Uint8Array(32) : hash32("cert-fingerprint");
const admin = await loadSigner("admin", process.env.ELOTE_ADMIN_KEYPAIR ?? ".demo-keys/admin.json");
const borrower = await loadSigner("borrower", required("borrower-keypair"));
const attestor = await roleSigner("attestor");

const pool = await loadPool(c, poolAddress);
if (pool.attestor !== attestor.address) fail(`the attestor key (${attestor.address}) is not this pool's attestor (${pool.attestor})`);
if (pool.paused) fail("the pool is paused: openings and draws are blocked until the admin resumes it");
const { line, data: l } = await loadLine(c, poolAddress, lineId);
if (l.borrower !== borrower.address) fail(`the borrower key (${borrower.address}) is not this line's borrower (${l.borrower})`);
if (l.status !== LineStatus.Approved && l.status !== LineStatus.Active) fail(`the line is ${LineStatus[l.status]}: only an Approved or Active line draws`);
if (amount > l.principal - l.drawn) fail(`the line has ${fmt(l.principal - l.drawn)} tMXN left to draw`);

const drawIndex = l.draws;
const borrowerAsset = await ataOf(borrower.address, pool.assetMint);
const signature = await sendOrFail(c, `draw ${fmt(amount)} tMXN, pagaré ${drawIndex}`, borrower, [
  await getCreateAssociatedTokenIdempotentInstructionAsync({ payer: borrower, owner: borrower.address, mint: pool.assetMint }),
  await getDrawInstructionAsync({
    borrower,
    attestor,
    pool: poolAddress,
    line,
    assetMint: pool.assetMint,
    vault: pool.vault,
    borrowerAssetAccount: borrowerAsset,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
    payer: admin,
    ...(await drawPagareAccounts({ line, drawIndex, collection: pool.collection })),
    systemProgram: SYSTEM_PROGRAM_ADDRESS,
    amount,
    documentHash,
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
    `amount       ${fmt(pagare.amount)} tMXN; drawn through ${fmt(pagare.drawnThrough)}`,
    `due          ${new Date(Number(pagare.dueAt) * 1000).toISOString()}`,
    `line now     drawn ${fmt(after.drawn)} of ${fmt(after.principal)}, interest ${fmt(after.interestDue)}, ${LineStatus[after.status]}`,
    `signature    ${signature}`,
    `explorer     ${explorer(c, signature)}`,
  ].join("\n")
);
console.log(`\n${DISCLAIMER}`);
