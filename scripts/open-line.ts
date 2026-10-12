// Opens one credit line on an existing pool for a borrower whose key is held locally (for example
// a devnet demo key that is later imported into a wallet to rehearse the borrower's draw and
// repayments in the app). open_line is signed by the pool admin and the borrower in the same
// transaction (and by the sponsor when one is named); it moves no funds.
//
//   npx tsx scripts/open-line.ts --cluster local|devnet --pool <address> --borrower-keypair <path>
//                                --principal <tMXN> [--rate-bps <n>] [--term-days <n>]
//                                [--installments <n> --interval-days <n> [--late-rate-bps <n>] [--max-missed <n>]]
//                                [--sponsor-keypair <path>] [--agreement-hash <64 hex>]
//
// --principal       amount in tMXN, up to 6 decimals (500, 1250.5)
// --rate-bps        fixed interest on the principal, in basis points (default 1200)
// --term-days       days from now to the end of the term (default 90)
// --installments    open a scheduled line (version 2) with this many installments; needs --interval-days
// --interval-days   days between installments; due dates count from the draw, so the term must be
//                   longer than installments * interval
// --late-rate-bps   annual late interest on overdue amounts (default 10000, as in the demo)
// --max-missed      missed installments before collection (default 3, the program's minimum)
// --sponsor-keypair the sponsor co-signs and is named on the line
// --agreement-hash  hash of the signed credit agreement (placeholder of the demo if absent)
//
// The admin key comes from ELOTE_ADMIN_KEYPAIR or .demo-keys/admin.json; ELOTE_RPC overrides the
// cluster's RPC URL. Keys are never printed, only addresses.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  type Address,
  type KeyPairSigner,
  address,
  appendTransactionMessageInstructions,
  createKeyPairSignerFromBytes,
  createSolanaRpc,
  createTransactionMessage,
  getBase64EncodedWireTransaction,
  getSignatureFromTransaction,
  isSolanaError,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
} from "@solana/kit";
import { fetchToken } from "@solana-program/token";
import {
  type ElotePoolError,
  LineStatus,
  decodeSchedule,
  fetchCreditLine,
  fetchMaybePool,
  findLinePda,
  findPoolPda,
  getElotePoolErrorMessage,
  getOpenLineInstructionAsync,
  isElotePoolError,
} from "../clients/js/src/index.ts";

const DISCLAIMER = "Demo on Solana devnet with test assets. Not an offer of securities.";
const ASSET = "tMXN (devnet test token, no value)";
const DECIMALS = 6;
const UNIT = 10n ** BigInt(DECIMALS);
const DAY = 86_400n;
const MIN_MAX_MISSED = 3; // state::MIN_MAX_MISSED
const MAX_INSTALLMENTS = 120; // state::MAX_INSTALLMENTS

function fail(message: string): never {
  console.error(`open-line: ${message}`);
  process.exit(1);
}

// -- arguments ------------------------------------------------------------------------------------

const FLAGS = [
  "cluster",
  "pool",
  "borrower-keypair",
  "principal",
  "rate-bps",
  "term-days",
  "installments",
  "interval-days",
  "late-rate-bps",
  "max-missed",
  "sponsor-keypair",
  "agreement-hash",
];
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i += 2) {
  const name = argv[i].startsWith("--") ? argv[i].slice(2) : "";
  if (!FLAGS.includes(name)) fail(`unknown argument ${JSON.stringify(argv[i])}; flags take one value each (--${FLAGS.join(", --")})`);
}

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(`--${name}`);
  if (i < 0) return undefined;
  const value = process.argv[i + 1];
  if (value === undefined || value.startsWith("--")) fail(`--${name} needs a value`);
  return value;
}
function required(name: string): string {
  return arg(name) ?? fail(`--${name} is required`);
}
function integer(name: string, fallback: number | undefined, min: number, max: number): number | undefined {
  const raw = arg(name);
  if (raw === undefined) return fallback;
  if (!/^\d+$/.test(raw)) fail(`--${name} must be a whole number`);
  const n = Number(raw);
  if (n < min || n > max) fail(`--${name} must be between ${min} and ${max}`);
  return n;
}

const cluster = required("cluster");
if (cluster !== "local" && cluster !== "devnet") fail("--cluster must be local or devnet");
let poolAddress: Address;
try {
  poolAddress = address(required("pool"));
} catch {
  fail("--pool is not a valid address");
}
const borrowerFile = required("borrower-keypair");
const principal = parseAmount(required("principal"));
const rateBps = integer("rate-bps", 1_200, 0, 0xffff)!;
const termDays = integer("term-days", 90, 1, 36_500)!;
const installments = integer("installments", undefined, 1, MAX_INSTALLMENTS);
const intervalDays = integer("interval-days", undefined, 1, 36_500);
const lateBps = integer("late-rate-bps", 10_000, 0, 0xffff)!;
const maxMissed = integer("max-missed", MIN_MAX_MISSED, MIN_MAX_MISSED, 0xff)!;
const sponsorFile = arg("sponsor-keypair");

if ((installments === undefined) !== (intervalDays === undefined)) {
  fail("--installments and --interval-days go together");
}
if (installments === undefined && (arg("late-rate-bps") !== undefined || arg("max-missed") !== undefined)) {
  fail("--late-rate-bps and --max-missed apply to a scheduled line only (add --installments and --interval-days)");
}
const schedule =
  installments !== undefined && intervalDays !== undefined
    ? { intervalSeconds: BigInt(intervalDays) * DAY, installments, lateBps, maxMissed }
    : null;
if (schedule) {
  if (schedule.installments < schedule.maxMissed) {
    fail(
      `a schedule needs at least --max-missed installments (${schedule.maxMissed}); the program requires ` +
        `${MIN_MAX_MISSED} <= max-missed <= installments`
    );
  }
  // The last due date is checked at the opening and again at the draw, counted from the draw.
  const spanDays = intervalDays! * installments!;
  if (termDays <= spanDays) {
    fail(
      `the schedule spans ${spanDays} days from the draw, so --term-days must be more than ${spanDays} ` +
        `(leave room for the draw, for example ${spanDays + 30})`
    );
  }
}

/** Decimal tMXN to base units, without floating point. */
function parseAmount(raw: string): bigint {
  const m = /^(\d+)(?:\.(\d{1,6}))?$/.exec(raw);
  if (!m) fail(`--principal must be a positive amount with at most ${DECIMALS} decimals`);
  const units = BigInt(m[1]) * UNIT + BigInt((m[2] ?? "").padEnd(DECIMALS, "0"));
  if (units === 0n) fail("--principal must be more than zero");
  if (units > 0xffff_ffff_ffff_ffffn) fail("--principal does not fit in a u64");
  return units;
}

function agreementHash(): Uint8Array {
  const given = arg("agreement-hash");
  if (given) {
    const hex = given.replace(/^0x/, "");
    if (!/^[0-9a-fA-F]{64}$/.test(hex)) fail("--agreement-hash must be 32 bytes of hex");
    if (/^0+$/.test(hex)) fail("--agreement-hash must not be all zeros");
    return Uint8Array.from(Buffer.from(hex, "hex"));
  }
  console.log("No --agreement-hash given: using a placeholder hash of a fixed demo text.");
  return Uint8Array.from(createHash("sha256").update("elote.fun demo credit agreement").digest());
}

const rpcUrl = process.env.ELOTE_RPC ?? (cluster === "devnet" ? "https://api.devnet.solana.com" : "http://127.0.0.1:8899");
const rpc = createSolanaRpc(rpcUrl);

// -- keys -----------------------------------------------------------------------------------------

/** Loads an existing 64-byte Solana keypair file. Never printed, never created here. */
async function loadSigner(label: string, file: string): Promise<KeyPairSigner> {
  const resolved = path.resolve(file.replace(/^~/, os.homedir()));
  if (!fs.existsSync(resolved)) fail(`${label} keypair file not found`);
  let bytes: Uint8Array;
  try {
    bytes = Uint8Array.from(JSON.parse(fs.readFileSync(resolved, "utf8")));
  } catch {
    fail(`${label} keypair file is not a JSON array of bytes`);
  }
  if (bytes.length !== 64) fail(`${label} keypair file must hold 64 bytes`);
  try {
    return await createKeyPairSignerFromBytes(bytes);
  } catch {
    fail(`${label} keypair file does not hold a valid keypair`);
  }
}

// -- errors ---------------------------------------------------------------------------------------

/** The program's error code in a failed send, if the failure came from elote_pool. */
function programError(error: unknown, message: { instructions: Record<number, { programAddress: Address }> }): string | null {
  for (let e: unknown = error; e; e = (e as { cause?: unknown }).cause) {
    if (isElotePoolError(e, message)) {
      const code = e.context.code as ElotePoolError;
      return `program error ${code}: ${getElotePoolErrorMessage(code)}`;
    }
    if (!isSolanaError(e)) break;
  }
  return null;
}

/** Same for the JSON `err` of a signature status: `{ InstructionError: [index, { Custom: code }] }`. */
function statusError(err: unknown): string {
  const custom = (err as { InstructionError?: [number, { Custom?: number }] })?.InstructionError?.[1]?.Custom;
  if (typeof custom === "number" && custom >= 6000) {
    return `program error ${custom}: ${getElotePoolErrorMessage(custom as ElotePoolError)}`;
  }
  return JSON.stringify(err, (_, v) => (typeof v === "bigint" ? v.toString() : v));
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function explorer(signature: string): string {
  const c = cluster === "devnet" ? "devnet" : `custom&customUrl=${encodeURIComponent(rpcUrl)}`;
  return `https://explorer.solana.com/tx/${signature}?cluster=${c}`;
}

const fmt = (units: bigint) => {
  const whole = units / UNIT;
  const frac = (units % UNIT).toString().padStart(DECIMALS, "0").replace(/0+$/, "");
  return `${whole.toLocaleString("en-US")}${frac ? "." + frac : ""}`;
};

// -- open the line --------------------------------------------------------------------------------

async function main() {
  console.log(`${DISCLAIMER}\nCluster: ${cluster} (${rpcUrl})\n`);
  const hash = agreementHash();
  const adminFile = process.env.ELOTE_ADMIN_KEYPAIR ?? path.join(".demo-keys", "admin.json");
  const admin = await loadSigner("admin", adminFile);
  const borrower = await loadSigner("borrower", borrowerFile);
  const sponsor = sponsorFile ? await loadSigner("sponsor", sponsorFile) : null;

  const maybePool = await fetchMaybePool(rpc, poolAddress, { commitment: "confirmed" });
  if (!maybePool.exists) fail(`no elote_pool pool at ${poolAddress} on ${cluster}`);
  const pool = maybePool.data;
  const [derived] = await findPoolPda({ admin: pool.admin, assetMint: pool.assetMint });
  if (derived !== poolAddress) fail("the account is not a pool at its expected address");
  if (pool.admin !== admin.address) fail(`the admin key (${admin.address}) is not this pool's admin (${pool.admin})`);
  if (pool.paused) fail("the pool is paused: openings and draws are blocked until the admin resumes it");

  const { value: adminLamports } = await rpc.getBalance(admin.address, { commitment: "confirmed" }).send();
  if (adminLamports < 10_000_000n) fail("the admin needs at least 0.01 SOL for the line account's rent and the fee");

  const lineId = pool.lineCount;
  const [line] = await findLinePda({ pool: poolAddress, lineId });
  const termEnd = BigInt(Math.floor(Date.now() / 1000)) + BigInt(termDays) * DAY;
  const vault = await fetchToken(rpc, pool.vault, { commitment: "confirmed" });

  const instruction = await getOpenLineInstructionAsync({
    admin,
    borrower,
    ...(sponsor ? { sponsor } : {}),
    pool: poolAddress,
    line,
    lineId,
    principal,
    rateBps,
    termEnd,
    agreementHash: hash,
    sponsorArg: sponsor ? sponsor.address : null,
    schedule,
  });
  const { value: blockhash } = await rpc.getLatestBlockhash({ commitment: "confirmed" }).send();
  const message = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayerSigner(admin, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
    (m) => appendTransactionMessageInstructions([instruction], m)
  );
  const tx = await signTransactionMessageWithSigners(message);
  const signature = getSignatureFromTransaction(tx);
  try {
    await rpc
      .sendTransaction(getBase64EncodedWireTransaction(tx), { encoding: "base64", preflightCommitment: "confirmed" })
      .send();
  } catch (error) {
    fail(`open_line was rejected: ${programError(error, message) ?? (error as Error).message}`);
  }
  let confirmed = false;
  for (let i = 0; i < 60 && !confirmed; i++) {
    await sleep(1_000);
    const {
      value: [status],
    } = await rpc.getSignatureStatuses([signature]).send();
    if (status?.err) fail(`open_line failed (${signature}): ${statusError(status.err)}`);
    confirmed = status?.confirmationStatus === "confirmed" || status?.confirmationStatus === "finalized";
  }
  if (!confirmed) fail(`open_line not confirmed in time (${signature}); check the explorer before retrying`);

  const opened = (await fetchCreditLine(rpc, line, { commitment: "confirmed" })).data;
  const s = decodeSchedule(opened);
  const lines = [
    `cluster     ${cluster}`,
    `pool        ${poolAddress}`,
    `asset       ${pool.assetMint} (${ASSET})`,
    `line id     ${opened.lineId}`,
    `line        ${line}`,
    `borrower    ${opened.borrower}`,
    `sponsor     ${opened.sponsor.__option === "Some" ? opened.sponsor.value : "none"}`,
    `principal   ${fmt(opened.principal)} tMXN`,
    `rate        ${opened.rateBps} bps (interest ${fmt(opened.interestDue)} tMXN)`,
    `term        ${termDays} days, ends ${new Date(Number(opened.termEnd) * 1000).toISOString()}`,
    `version     ${opened.version}${s ? " (scheduled)" : " (single maturity)"}`,
    s
      ? `schedule    ${s.installments} installments every ${s.intervalSeconds / DAY} days from the draw, late interest ${s.lateBps} bps a year, collection after ${s.maxMissed} missed`
      : `schedule    none`,
    `status      ${LineStatus[opened.status]}`,
    `signature   ${signature}`,
    `explorer    ${explorer(signature)}`,
  ];
  console.log(lines.join("\n"));
  if (opened.principal > vault.data.amount) {
    console.log(`\nNote: the vault holds ${fmt(vault.data.amount)} tMXN; the draw needs ${fmt(opened.principal)} tMXN of liquidity.`);
  }
  console.log(
    `\nThe borrower draws from its own wallet: it needs some SOL for fees and a token account for the ${ASSET}.`
  );
  console.log(`\n${DISCLAIMER}`);
}

await main();
