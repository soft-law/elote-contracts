// Opens one credit line on an existing pool for a borrower whose key is held locally (for example
// a demo key later imported into a wallet to rehearse the borrower's draw and repayments in the
// app). open_line is signed by the pool admin, the borrower and the pool's attestor in the same
// transaction (and by the sponsor when one is named); it moves no funds.
//
//   npx tsx scripts/open-line.ts --cluster local|devnet --pool <address> --borrower-keypair <path>
//        --principal <tMXN> [--rate-bps <n>] [--term-days <n>]
//        [--installments <n> --interval-days <n> [--late-rate-bps <n>] [--max-missed <n>]]
//        [--sponsor-keypair <path> [--sponsor-role promoter|guarantor]]
//        [--agreement-hash <64 hex>] [--expediente-root <64 hex>] [--attestor-keypair <path>] [--yes]
//
// --principal        amount in tMXN, up to 6 decimals (500, 1250.5)
// --rate-bps         annual interest, charged per tranche at the draw (default 1200)
// --term-days        days from now to the end of the term (default 90)
// --installments     open a scheduled line with this many installments; needs --interval-days
// --interval-days    days between installments; the first falls due one interval from now
// --late-rate-bps    annual late interest on overdue principal (default 10000)
// --max-missed       missed installments before collection (default 3, the program's minimum)
// --sponsor-keypair  the sponsor co-signs and is named on the line
// --sponsor-role     promoter (no payment obligation, the default) or guarantor (full liability)
// --agreement-hash   hash of the signed credit agreement (a placeholder if absent)
// --expediente-root  salted merkle root of the credit file (a placeholder if absent)
// --attestor-keypair the pool's attestor (default ELOTE_ATTESTOR_KEYPAIR or .demo-keys/attestor.json,
//                    the key demo-pool.ts creates); it stands in for the agreements engine
// --yes              send without asking when the cluster is not this machine
//
// The admin key comes from ELOTE_ADMIN_KEYPAIR or .demo-keys/admin.json; ELOTE_RPC overrides the
// cluster's RPC URL. Keys are never printed, only addresses.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { address } from "@solana/kit";
import { fetchToken } from "@solana-program/token";
import {
  LineStatus,
  fetchCreditLine,
  findLinePda,
  getOpenLineInstructionAsync,
  lineSchedule,
  maxLinePrincipal,
} from "../clients/js/src/index.ts";
import {
  ASSET,
  DAY,
  DISCLAIMER,
  arg,
  checkFlags,
  cluster,
  confirmCluster,
  explorer,
  fail,
  fmt,
  hash32,
  integer,
  loadSigner,
  parseAmount,
  required,
  roleSigner,
  sendOrFail,
} from "./lib/cli.ts";
import { loadPool } from "./lib/pool.ts";

const MIN_MAX_MISSED = 3; // state::MIN_MAX_MISSED
const MAX_INSTALLMENTS = 120; // state::MAX_INSTALLMENTS
const SPONSOR_ROLES = { promoter: 0, guarantor: 1 } as const; // state::SPONSOR_*

checkFlags([
  "cluster", "pool", "borrower-keypair", "principal", "rate-bps", "term-days", "installments", "interval-days",
  "late-rate-bps", "max-missed", "sponsor-keypair", "sponsor-role", "agreement-hash", "expediente-root",
  "attestor-keypair", "yes",
]);
const c = cluster();
const poolAddress = address(required("pool"));
const borrowerFile = required("borrower-keypair");
const principal = parseAmount("principal", required("principal"));
const rateBps = integer("rate-bps", 1_200, 0, 0xffff)!;
const termDays = integer("term-days", 90, 1, 36_500)!;
const installments = integer("installments", undefined, 1, MAX_INSTALLMENTS);
const intervalDays = integer("interval-days", undefined, 1, 36_500);
const lateBps = integer("late-rate-bps", 10_000, 0, 10_000)!;
const maxMissed = integer("max-missed", MIN_MAX_MISSED, MIN_MAX_MISSED, 0xff)!;
const sponsorFile = arg("sponsor-keypair");
const roleName = arg("sponsor-role") ?? "promoter";
if (!(roleName in SPONSOR_ROLES)) fail("--sponsor-role must be promoter or guarantor");
if (arg("sponsor-role") !== undefined && !sponsorFile) fail("--sponsor-role needs --sponsor-keypair");
const sponsorRole = SPONSOR_ROLES[roleName as keyof typeof SPONSOR_ROLES];
if ((installments === undefined) !== (intervalDays === undefined)) fail("--installments and --interval-days go together");
if (installments === undefined && (arg("late-rate-bps") !== undefined || arg("max-missed") !== undefined)) {
  fail("--late-rate-bps and --max-missed apply to a scheduled line only (add --installments and --interval-days)");
}
if (installments !== undefined && maxMissed > installments) fail("--max-missed cannot exceed --installments");

// The calendar comes from the signed contract; this script stands in for the agreements engine
// and puts the first due date one interval from now.
const nowSeconds = BigInt(Math.floor(Date.now() / 1000));
const termEnd = nowSeconds + BigInt(termDays) * DAY;
const schedule =
  installments === undefined
    ? null
    : {
        firstDueAt: nowSeconds + BigInt(intervalDays!) * DAY,
        intervalSeconds: BigInt(intervalDays!) * DAY,
        installments,
        lateBps,
        maxMissed,
      };
if (schedule && schedule.firstDueAt + BigInt(installments! - 1) * schedule.intervalSeconds > termEnd) {
  fail("the last installment would fall due after the term: raise --term-days or lower --installments or --interval-days");
}

await confirmCluster(c, "open_line");
const agreementHash = hash32("agreement-hash", "elote.fun demo credit agreement");
const expedienteRoot = hash32("expediente-root", "elote.fun demo credit file");
const admin = await loadSigner("admin", process.env.ELOTE_ADMIN_KEYPAIR ?? ".demo-keys/admin.json");
const borrower = await loadSigner("borrower", borrowerFile);
const attestor = await roleSigner("attestor");
const sponsor = sponsorFile ? await loadSigner("sponsor", sponsorFile) : null;

const pool = await loadPool(c, poolAddress);
if (pool.admin !== admin.address) fail(`the admin key (${admin.address}) is not this pool's admin (${pool.admin})`);
if (pool.attestor !== attestor.address) fail(`the attestor key (${attestor.address}) is not this pool's attestor (${pool.attestor})`);
if (pool.paused) fail("the pool is paused: openings and draws are blocked until the admin resumes it");
const { value: adminLamports } = await c.rpc.getBalance(admin.address, { commitment: "confirmed" }).send();
if (adminLamports < 10_000_000n) fail("the admin needs at least 0.01 SOL for the line account's rent and the fee");

const lineId = pool.lineCount;
const [line] = await findLinePda({ pool: poolAddress, lineId });
const vault = await fetchToken(c.rpc, pool.vault, { commitment: "confirmed" });
// The concentration cap, checked before signing; LineTooLarge on chain is the backstop.
const cap = maxLinePrincipal(vault.data.amount + pool.outstandingPrincipal, pool.maxLineBps);
if (principal > cap) {
  fail(`the principal (${fmt(principal)}) exceeds this pool's largest line: ${pool.maxLineBps} bps of the NAV, ${fmt(cap)} right now`);
}

const signature = await sendOrFail(c, "open_line", admin, [
  await getOpenLineInstructionAsync({
    admin,
    borrower,
    ...(sponsor ? { sponsor } : {}),
    attestor,
    pool: poolAddress,
    line,
    vault: pool.vault,
    lineId,
    principal,
    rateBps,
    termEnd,
    agreementHash,
    sponsorArg: sponsor ? sponsor.address : null,
    sponsorRole: sponsor ? sponsorRole : 0,
    schedule,
    expedienteRoot,
  }),
]);

const opened = (await fetchCreditLine(c.rpc, line, { commitment: "confirmed" })).data;
const s = lineSchedule(opened);
console.log(
  [
    `cluster      ${c.name}`,
    `pool         ${poolAddress}`,
    `asset        ${pool.assetMint} (${ASSET})`,
    `line id      ${opened.lineId}`,
    `line         ${line}`,
    `borrower     ${opened.borrower}`,
    `sponsor      ${opened.sponsor.__option === "Some" ? `${opened.sponsor.value} (${roleName})` : "none"}`,
    `principal    ${fmt(opened.principal)} tMXN`,
    `rate         ${opened.rateBps} bps a year, charged per tranche at the draw`,
    `term         ${termDays} days, ends ${new Date(Number(opened.termEnd) * 1000).toISOString()}`,
    `draws until  ${new Date(Number(opened.availabilityEnd) * 1000).toISOString()}`,
    s
      ? `schedule     ${s.installments} installments every ${s.intervalSeconds / DAY} days from ${new Date(Number(opened.firstDueAt) * 1000).toISOString()}, late interest ${s.lateBps} bps a year, collection after ${s.maxMissed} missed`
      : `schedule     none (bullet)`,
    `status       ${LineStatus[opened.status]}`,
    `signature    ${signature}`,
    `explorer     ${explorer(c, signature)}`,
  ].join("\n")
);
if (opened.principal > vault.data.amount) {
  console.log(`\nNote: the vault holds ${fmt(vault.data.amount)} tMXN; the line needs ${fmt(opened.principal)} tMXN of liquidity to be drawn in full.`);
}
console.log(`\n${DISCLAIMER}`);
