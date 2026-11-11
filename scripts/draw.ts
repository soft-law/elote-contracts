// Draws one tranche of an approved line: the borrower receives `--amount` and the program mints the
// tranche's pagaré (a frozen Metaplex Core asset owned by the pool, in the pool's collection) and its
// record. The borrower signs alone, over the FIX in force, which the script prints with the USDC
// to be delivered before asking; the admin pays the two accounts' rent as the app's relayer would.
// The pagaré hash is derived on chain from the approved template and terms. The borrower subscribes
// the pagaré by signing a message with its full text (decision 55); the draw records the message's
// SHA-256, the signature and today's date in Mexico City. When the pool names cotejo keys
// (decision 56), one of them co-signs: the lender's cotejo service, after checking the text, the hash
// and the signature. The draw also records the sealed copy's hash, or zero (decision 57).
//
//   npx tsx scripts/draw.ts --cluster local|devnet --pool <address> --line-id <n>
//        --borrower-keypair <path> --amount <MXN> [--signature-method wallet|psc|efirma
//        --cert-fingerprint <64 hex> --cert-salt <64 hex>] [--pagare-text <path>]
//        [--cotejo-keypair <path>] [--sealed-hash <64 hex>] [--yes]
//
// --amount            the tranche in MXN pesos, up to 2 decimals, at most what the line has left
// --signature-method  how the pagaré text was signed: wallet (the default), psc or efirma
// --cert-fingerprint  the signing certificate's fingerprint; with psc or efirma, and only with
// --cert-salt         the salt kept in the expediente: the draw records sha256(salt || fingerprint),
//                     never the fingerprint, which can be looked up to the signer's RFC
// --pagare-text       the file holding the filled pagaré text the borrower signs, as bytes; without
//                     it the borrower signs a placeholder labelled DEMO, which is no pagaré text.
//                     A file that is not plain UTF-8 text (no control character but \n, so no
//                     CRLF or tab, no BOM or invisible mark) is refused before anything is signed
// --cotejo-keypair    the cotejo key that co-signs; required when the pool names any (its key must
//                     be one of them), ignored by the program otherwise
// --sealed-hash       SHA-256 of the encrypted content of the sealed copy the parties keep,
//                     recipients excluded; zero when omitted (no sealed copy)
// --yes               send without asking when the cluster is not this machine
//
// The admin key comes from ELOTE_ADMIN_KEYPAIR or .demo-keys/admin.json; ELOTE_RPC overrides the
// cluster's RPC URL. Keys are never printed, only addresses.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { readFileSync } from "node:fs";
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
  poolCotejoKeys,
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
import { ataOf, loadLine, loadPool, signPagare } from "./lib/pool.ts";

const METHODS = { wallet: 0, psc: 1, efirma: 2 } as const; // state::SIGNATURE_*

checkFlags([
  "cluster", "pool", "line-id", "borrower-keypair", "amount", "signature-method", "cert-fingerprint", "cert-salt",
  "pagare-text", "cotejo-keypair", "sealed-hash", "yes",
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
const cotejo = arg("cotejo-keypair") === undefined ? undefined : await loadSigner("cotejo", arg("cotejo-keypair")!);
const sealedHash = arg("sealed-hash") === undefined ? new Uint8Array(32) : hash32("sealed-hash");

const pool = await loadPool(c, poolAddress);
if (pool.fixRate === 0n) fail("no FIX is published on this pool: the council or the feeder posts it first");
if (pool.paused) fail("the pool is paused: requests, approvals and draws are blocked until the council resumes it");
const { line, data: l } = await loadLine(c, poolAddress, lineId);
if (l.borrower !== borrower.address) fail(`the borrower key (${borrower.address}) is not this line's borrower (${l.borrower})`);
if (l.status !== LineStatus.Approved && l.status !== LineStatus.Active) fail(`the line is ${LineStatus[l.status]}: only an Approved or Active line draws`);
const cotejoKeys = poolCotejoKeys(pool);
if (cotejoKeys.length > 0 && cotejo === undefined) fail(`the pool names cotejo keys (${cotejoKeys.join(", ")}): one of them co-signs the draw (--cotejo-keypair)`);
if (cotejoKeys.length > 0 && !cotejoKeys.includes(cotejo!.address)) fail(`${cotejo!.address} is not one of the pool's cotejo keys (${cotejoKeys.join(", ")})`);
if (cotejoKeys.length === 0 && cotejo !== undefined) console.log("the pool names no cotejo key: the co-signature is ignored");
if (amount > l.principal - l.drawn) fail(`the line has MXN ${fmtMxn(l.principal - l.drawn)} left to draw`);

const drawIndex = l.draws;
// The borrower confirms the FIX and the USDC shown here; a FIX published in between fails the draw
// (FixMismatch) and the script must be run again.
const usdc = usdcForMxn(amount, pool.fixRate);
console.log(`FIX ${pool.fixRate} (x10^4 MXN per USD) of ${pool.fixDate}: MXN ${fmtMxn(amount)} delivered as ${fmt(usdc)} USDC`);
const textPath = arg("pagare-text");
const signed = await signPagare(c, borrower, { line, lineId, drawIndex, message: textPath === undefined ? undefined : readFileSync(textPath) });
console.log(`pagaré text ${textPath ?? "DEMO placeholder"}: sha256 ${Buffer.from(signed.signedPagareHash).toString("hex")}, signed on ${signed.signedOn}`);
await confirmCluster(c, "draw");
const borrowerAsset = await ataOf(borrower.address, pool.assetMint);
const signature = await sendOrFail(c, `draw MXN ${fmtMxn(amount)}, pagaré ${drawIndex}`, borrower, [
  await getCreateAssociatedTokenIdempotentInstructionAsync({ payer: borrower, owner: borrower.address, mint: pool.assetMint }),
  await getDrawInstructionAsync({
    borrower,
    cotejo,
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
    fixRate: pool.fixRate,
    fixDate: pool.fixDate,
    certFingerprint,
    signatureMethod,
    ...signed,
    sealedHash,
  }),
]);

const after = (await fetchCreditLine(c.rpc, line, { commitment: "confirmed" })).data;
const [record] = await findPagarePda({ line, drawIndex });
const pagare = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
console.log(
  [
    `line         ${line} (id ${lineId})`,
    `pagaré       ${drawIndex}: asset ${pagare.asset}, record ${record}`,
    `cotejo       ${pagare.cotejo === "11111111111111111111111111111111" ? "none required" : pagare.cotejo}; sealed hash ${Buffer.from(pagare.sealedHash).toString("hex")}`,
    `amount       MXN ${fmtMxn(pagare.amountMxn)} as ${fmt(pagare.usdcDelivered)} USDC; drawn through MXN ${fmtMxn(pagare.drawnThrough)}`,
    `due          ${new Date(Number(pagare.dueAt) * 1000).toISOString()}`,
    `line now     drawn MXN ${fmtMxn(after.drawn)} of ${fmtMxn(after.principal)}, interest accrued on what is outstanding (decision 48), ${LineStatus[after.status]}`,
    `signature    ${signature}`,
    `explorer     ${explorer(c, signature)}`,
  ].join("\n")
);
console.log(`\n${DISCLAIMER}`);
