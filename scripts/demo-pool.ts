// End-to-end demo of the elote_pool program: two investors deposit, a credit line is approved,
// drawn and repaid (principal by the borrower, interest by a sponsor), and the value of every
// share rises with the collected interest. With --schedule, a second line is repaid in six
// installments: one is missed and the next repayment collects late interest first.
//
//   npx tsx scripts/demo-pool.ts --cluster local|devnet --multisig <address> [--agreement-hash <64 hex>] [--sponsor]
//        [--schedule [--interval-seconds <n>]] [--grace-seconds <n>] [--max-line-bps <n>] [--yes]
//
// --agreement-hash    hash of the signed credit agreement recorded on the line (placeholder if absent)
// --sponsor           the sponsor co-signs the opening and is named on the line
// --schedule          also open a 6-installment line, miss one installment and repay it with late interest
// --interval-seconds  seconds between installments (default: one month, 2,592,000; 20 on local, a
//                     compressed "demo month", since a validator's clock cannot be moved forward)
// --grace-seconds     the pool's grace period (default 0; 5 with --schedule on local)
// --max-line-bps      largest line as a share of the NAV, in basis points (default 5000 here, so
//                     the 6,000 line fits the 15,000 of deposits; production pools use 1000)
// --yes               send without asking when the cluster is not this machine
//
// Keys live in .demo-keys/ (generated on first run, ignored by git) unless a path is given in
// ELOTE_ADMIN_KEYPAIR, ELOTE_INVESTOR_A_KEYPAIR, ELOTE_INVESTOR_B_KEYPAIR, ELOTE_BORROWER_KEYPAIR or
// ELOTE_SPONSOR_KEYPAIR. Only addresses are printed. ELOTE_RPC overrides the cluster's RPC URL.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { type Address, type Instruction, address, type KeyPairSigner, generateKeyPairSigner, lamports } from "@solana/kit";
import { SYSTEM_PROGRAM_ADDRESS, getCreateAccountInstruction, getTransferSolInstruction } from "@solana-program/system";
import {
  TOKEN_PROGRAM_ADDRESS,
  fetchMint,
  fetchToken,
  getCreateAssociatedTokenIdempotentInstructionAsync,
  getInitializeMint2Instruction,
  getMintSize,
  getMintToInstruction,
} from "@solana-program/token";
import {
  LineStatus,
  drawPagareAccounts,
  fetchCreditLine,
  fetchPool,
  findDeadSharesPda,
  findLinePda,
  findPoolPda,
  findVaultPda,
  getCreateCollectionV2Instruction,
  getDepositInstructionAsync,
  getDrawInstructionAsync,
  getInitPoolInstructionAsync,
  getRequestLineInstructionAsync,
  getRedeemInstructionAsync,
  lineArrears,
  lineSchedule,
} from "../clients/js/src/index.ts";
import {
  ASSET,
  DISCLAIMER,
  UNIT,
  arg,
  chainNow,
  checkFlags,
  cluster,
  confirmCluster,
  demoSigner,
  fail,
  flag,
  fmt,
  hash32,
  sendOrFail,
  sleep,
} from "./lib/cli.ts";
import { ataOf, repayInParts } from "./lib/pool.ts";

checkFlags(["cluster", "multisig", "agreement-hash", "sponsor", "schedule", "interval-seconds", "grace-seconds", "max-line-bps", "yes"]);
const c = cluster();
const withSponsor = flag("sponsor");
const withSchedule = flag("schedule");
const MONTH_SECONDS = 30n * 86_400n;
const intervalSeconds = BigInt(arg("interval-seconds") ?? (c.local ? "20" : MONTH_SECONDS.toString()));
const graceSeconds = Number(arg("grace-seconds") ?? (withSchedule && c.local ? "5" : "0"));
// The council: vault 0 of an existing autonomous Squads v4 multisig with a majority threshold. The
// admin key below cannot sign as it, so the steps after init_pool need the council tooling (PR 4).
const multisig = address(arg("multisig") ?? fail("--multisig <address> is required: the Squads v4 multisig whose vault 0 is the council"));
if (intervalSeconds <= 0n) fail("--interval-seconds must be positive");
if (!Number.isInteger(graceSeconds) || graceSeconds < 0 || graceSeconds > 0xffffffff) fail("--grace-seconds must fit in a u32");
// The concentration cap: production pools use 1,000 bps; the demo's 6,000 tMXN line is 40% of its
// 15,000 of deposits, so it defaults to 5,000.
const maxLineBps = Number(arg("max-line-bps") ?? "5000");
if (!Number.isInteger(maxLineBps) || maxLineBps < 1 || maxLineBps > 10_000) fail("--max-line-bps must be between 1 and 10000");

await confirmCluster(c, "the demo's transactions");
const hash = hash32("agreement-hash", "elote.fun demo credit agreement");
const send = (label: string, feePayer: KeyPairSigner, instructions: Instruction[]) => sendOrFail(c, label, feePayer, instructions);

const admin = await demoSigner("admin", "ELOTE_ADMIN_KEYPAIR");
const investorA = await demoSigner("investor-a", "ELOTE_INVESTOR_A_KEYPAIR");
const investorB = await demoSigner("investor-b", "ELOTE_INVESTOR_B_KEYPAIR");
const borrower = await demoSigner("borrower", "ELOTE_BORROWER_KEYPAIR");
const sponsor = await demoSigner("sponsor", "ELOTE_SPONSOR_KEYPAIR");
console.log(
  [
    `admin      ${admin.address}`,
    `investor A ${investorA.address}`,
    `investor B ${investorB.address}`,
    `borrower   ${borrower.address}`,
    `sponsor    ${sponsor.address}`,
  ].join("\n") + "\n"
);

async function airdrop(to: Address, sol: number): Promise<void> {
  const want = BigInt(sol * 1e9);
  for (let attempt = 1; attempt <= 6; attempt++) {
    try {
      const { value: balance } = await c.rpc.getBalance(to, { commitment: "confirmed" }).send();
      if (balance >= want / 2n) return;
      await c.rpc.requestAirdrop(to, lamports(want)).send();
      for (let i = 0; i < 30; i++) {
        await sleep(1_000);
        const { value } = await c.rpc.getBalance(to, { commitment: "confirmed" }).send();
        if (value > balance) return;
      }
    } catch (e) {
      console.log(`  airdrop attempt ${attempt} failed: ${(e as Error).message}`);
    }
    await sleep(attempt * 3_000);
  }
  fail(`could not fund ${to}; send it some SOL and run again`);
}

console.log("Funding the admin with SOL for fees and rent");
await airdrop(admin.address, c.name === "devnet" ? 1 : 10);
await send(
  "SOL for the other wallets' fees",
  admin,
  [investorA, investorB, borrower, sponsor].map((w) =>
    getTransferSolInstruction({ source: admin, destination: w.address, amount: lamports(20_000_000n) })
  )
);

// The test asset: a fresh 6-decimal SPL mint per run, so every run gets its own pool.
console.log(`\nCreating the ${ASSET}`);
const assetMint = await generateKeyPairSigner();
const mintSpace = BigInt(getMintSize());
const mintRent = await c.rpc.getMinimumBalanceForRentExemption(mintSpace).send();
await send(`create ${ASSET}`, admin, [
  getCreateAccountInstruction({ payer: admin, newAccount: assetMint, lamports: mintRent, space: mintSpace, programAddress: TOKEN_PROGRAM_ADDRESS }),
  getInitializeMint2Instruction({ mint: assetMint.address, decimals: 6, mintAuthority: admin.address }),
]);
console.log(`  ${ASSET} address ${assetMint.address}`);

const [pool] = await findPoolPda({ creator: admin.address, assetMint: assetMint.address });

// The share mint is supplied by the admin, with the pool PDA as mint authority and the asset's
// decimals. The demo uses a classic SPL mint, which the program accepts; a production series uses
// a Token-2022 mint frozen by default under Token ACL, so that `Pool.permissioned` is true.
const shareMintSigner = await generateKeyPairSigner();
const shareMint = shareMintSigner.address;
// The series' Metaplex Core collection: bare, with the pool PDA as its update authority, so that
// only the program can mint pagarés into it. It must exist before init_pool, which checks it.
const collectionSigner = await generateKeyPairSigner();
const collection = collectionSigner.address;
await send("create the share token and the pagarés' Core collection", admin, [
  getCreateAccountInstruction({ payer: admin, newAccount: shareMintSigner, lamports: mintRent, space: mintSpace, programAddress: TOKEN_PROGRAM_ADDRESS }),
  getInitializeMint2Instruction({ mint: shareMint, decimals: 6, mintAuthority: pool }),
  getCreateCollectionV2Instruction({ collection: collectionSigner, updateAuthority: pool, payer: admin, name: "ELOTE-DEMO", uri: "" }),
]);
const [vault] = await findVaultPda({ pool });
const [deadShares] = await findDeadSharesPda({ pool });
// The first deposit locks this many shares in the pool's own share account (state::DEAD_SHARES).
const DEAD_SHARES = 1_000n;

console.log("\nCreating the pool");
await send("init_pool", admin, [
  await getInitPoolInstructionAsync({
    creator: admin,
    multisig,
    assetMint: assetMint.address,
    pool,
    shareMint,
    vault,
    deadShares,
    collection,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
    shareTokenProgram: TOKEN_PROGRAM_ADDRESS,
    gracePeriodSeconds: graceSeconds,
    maxLineBps,
    collectionPeriodSeconds: 30 * 86_400,
    guardian: address("11111111111111111111111111111111"),
    maxOutstandingPrincipal: 2n ** 64n - 1n,
    accelerationNoticeSeconds: 7 * 86_400,
    cancelGraceSeconds: 30 * 86_400,
  }),
]);
console.log(
  `  pool ${pool}\n  share token ${shareMint}\n  collection ${collection}\n  vault ${vault}\n  grace period ${graceSeconds} s\n  max line ${maxLineBps} bps of the NAV`
);

console.log(`\nIssuing ${ASSET} to the investors and the sponsor`);
const issue: Instruction[] = [];
for (const [w, amount] of [
  [investorA, 10_000n * UNIT],
  [investorB, 5_000n * UNIT],
  [sponsor, 720n * UNIT],
] as const) {
  issue.push(await getCreateAssociatedTokenIdempotentInstructionAsync({ payer: admin, owner: w.address, mint: assetMint.address }));
  issue.push(getMintToInstruction({ mint: assetMint.address, token: await ataOf(w.address, assetMint.address), mintAuthority: admin, amount }));
}
issue.push(await getCreateAssociatedTokenIdempotentInstructionAsync({ payer: admin, owner: borrower.address, mint: assetMint.address }));
await send(`issue ${ASSET}: A 10,000, B 5,000, sponsor 720`, admin, issue);

async function snapshot() {
  const p = await fetchPool(c.rpc, pool, { commitment: "confirmed" });
  const v = await fetchToken(c.rpc, vault, { commitment: "confirmed" });
  const s = await fetchMint(c.rpc, shareMint, { commitment: "confirmed" });
  const nav = v.data.amount + p.data.outstandingPrincipal;
  const supply = s.data.supply;
  const perShare = supply === 0n ? 0n : (nav * UNIT) / supply;
  return { nav, supply, perShare, vault: v.data.amount, outstanding: p.data.outstandingPrincipal, interest: p.data.interestCollected };
}

/** Creates the depositor's share account if needed and deposits `assets` with an exact floor. */
async function deposit(label: string, who: KeyPairSigner, assets: bigint) {
  const { nav, supply } = await snapshot();
  const minSharesOut = supply === 0n ? assets - DEAD_SHARES : (assets * supply) / nav;
  await send(label, who, [
    await getCreateAssociatedTokenIdempotentInstructionAsync({ payer: who, owner: who.address, mint: shareMint }),
    await getDepositInstructionAsync({
      depositor: who,
      pool,
      assetMint: assetMint.address,
      shareMint,
      vault,
      tokenProgram: TOKEN_PROGRAM_ADDRESS,
      shareTokenProgram: TOKEN_PROGRAM_ADDRESS,
      depositorAssetAccount: await ataOf(who.address, assetMint.address),
      depositorShareAccount: await ataOf(who.address, shareMint),
      deadShares,
      assets,
      minSharesOut,
    }),
  ]);
}

console.log("\nDeposits");
await deposit(`investor A deposits 10,000 ${ASSET}`, investorA, 10_000n * UNIT);
await deposit(`investor B deposits 5,000 ${ASSET}`, investorB, 5_000n * UNIT);
const before = await snapshot();

console.log(`\nCredit line: 6,000 ${ASSET} at 12%${withSponsor ? ", co-signed by the sponsor" : ""}`);
const { lineCount } = (await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data;
const [line] = await findLinePda({ pool, lineId: lineCount });
const termEnd = BigInt(Math.floor(Date.now() / 1000) + 90 * 86_400);
// The council approves the request through Squads (PR 4 commit 10); until then the demo stops
// being runnable here.
await send("request_line", admin, [
  await getRequestLineInstructionAsync({
    borrower,
    payer: admin,
    ...(withSponsor ? { sponsor } : {}),
    pool,
    line,
    lineId: lineCount,
    principal: 6_000n * UNIT,
    rateBps: 1_200,
    lateBps: 10_000,
    termEnd,
    agreementHash: hash,
    sponsorArg: withSponsor ? sponsor.address : null,
    sponsorRole: 0,
    schedule: null,
    expedienteRoot: hash, // a placeholder for the salted root of the credit file
    pagareTemplateHash: hash,
  }),
]);
const borrowerAsset = await ataOf(borrower.address, assetMint.address);

/** A draw: the borrower receives `amount`; the admin pays the pagaré's rent as the app's relayer. */
async function drawIx(lineAddress: Address, drawIndex: number, amount: bigint) {
  return getDrawInstructionAsync({
    borrower,
    pool,
    line: lineAddress,
    assetMint: assetMint.address,
    vault,
    borrowerAssetAccount: borrowerAsset,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
    payer: admin,
    ...(await drawPagareAccounts({ line: lineAddress, drawIndex, collection })),
    systemProgram: SYSTEM_PROGRAM_ADDRESS,
    amountMxn: amount,
    fxRate: (await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data.fixRate,
    fxDate: (await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data.fixDate,
    certFingerprint: new Uint8Array(32), // wallet-only signature in the demo: no certificate
    signatureMethod: 0,
  });
}

await send(`draw: 6,000 ${ASSET} to the borrower, pagaré 0 minted`, borrower, [await drawIx(line, 0, 6_000n * UNIT)]);
const drawn = await snapshot();
const poolData = (await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data;
const noReceipt = new Uint8Array(32); // the demo has no receipt: "to follow"
console.log(`\nRepayments (interest is taken first)`);
await repayInParts(c, { pool, poolData, line, payer: borrower, amount: 6_000n * UNIT, receiptHash: noReceipt });
await repayInParts(c, { pool, poolData, line, payer: sponsor, amount: 720n * UNIT, receiptHash: noReceipt });
const repaid = await snapshot();
const status = (await fetchCreditLine(c.rpc, line, { commitment: "confirmed" })).data.status;

console.log("\nRedemption");
const redeemShares = 4_000n * UNIT;
await send("investor A redeems 4,000 shares", investorA, [
  await getRedeemInstructionAsync({
    holder: investorA,
    pool,
    assetMint: assetMint.address,
    shareMint,
    vault,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
    shareTokenProgram: TOKEN_PROGRAM_ADDRESS,
    holderShareAccount: await ataOf(investorA.address, shareMint),
    holderAssetAccount: await ataOf(investorA.address, assetMint.address),
    shares: redeemShares,
    minAssetsOut: (redeemShares * repaid.nav) / repaid.supply,
  }),
]);
const after = await snapshot();
const received = (redeemShares * repaid.nav) / repaid.supply;

const row = (label: string, s: Awaited<ReturnType<typeof snapshot>>) =>
  `${label.padEnd(26)}${fmt(s.nav).padStart(12)}${fmt(s.supply).padStart(12)}${fmt(s.perShare).padStart(14)}${fmt(s.interest).padStart(12)}`;
console.log(`\nAmounts in ${ASSET}; line status after repayment: ${LineStatus[status]}`);
console.log(`${"".padEnd(26)}${"NAV".padStart(12)}${"supply".padStart(12)}${"per share".padStart(14)}${"interest".padStart(12)}`);
console.log(row("after deposits", before));
console.log(row("line drawn", drawn));
console.log(row("line repaid", repaid));
console.log(row("after A redeems 4,000", after));
console.log(`\nInvestor A received ${fmt(received)} ${ASSET} for 4,000 shares (value per share ${fmt(before.perShare)} -> ${fmt(repaid.perShare)}).`);
console.log(`\npool ${pool}\nline 0 is repaid; its pagaré 0 is marked paid: cancel it with scripts/cancel-pagare.ts --line-id ${lineCount} --draw-index 0`);

if (withSchedule) await scheduleDemo();
console.log(`\n${DISCLAIMER}`);

// -- installments ---------------------------------------------------------------------------------

async function scheduleDemo() {
  const installments = 6;
  const principal = 600n * UNIT;
  const label = intervalSeconds === MONTH_SECONDS ? "monthly" : `every ${intervalSeconds} s (a compressed demo month)`;
  console.log(`\nInstallment line: 600 ${ASSET} at 12%, ${installments} installments ${label}, late interest 100% a year, collection after 3 missed`);
  const { lineCount: id } = (await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data;
  const [sLine] = await findLinePda({ pool, lineId: id });
  // The calendar comes from the contract: the first installment one interval from now, the draw
  // window open until then.
  const firstDueAt = (await chainNow(c)) + intervalSeconds;
  const sTermEnd = firstDueAt + BigInt(installments) * intervalSeconds;
  await send("request_line with a schedule", admin, [
    await getRequestLineInstructionAsync({
      borrower,
      payer: admin,
      pool,
      line: sLine,
      lineId: id,
      principal,
      rateBps: 1_200,
      termEnd: sTermEnd,
      agreementHash: hash,
      sponsorArg: null,
      sponsorRole: 0,
      lateBps: 10_000,
      schedule: { firstDueAt, intervalSeconds, installments, maxMissed: 3 },
      expedienteRoot: hash,
      pagareTemplateHash: hash,
    }),
  ]);
  await send(`draw: 600 ${ASSET} to the borrower, pagaré 0 minted`, borrower, [await drawIx(sLine, 0, principal)]);

  const grace = BigInt((await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data.gracePeriodSeconds);
  const read = async () => {
    const l = (await fetchCreditLine(c.rpc, sLine, { commitment: "confirmed" })).data;
    const s = lineSchedule(l)!;
    const now = await chainNow(c);
    return { l, s, now, a: lineArrears(l, s, grace, now) };
  };
  let { l } = await read();
  const installment = (l.drawn + l.interestDue) / BigInt(installments);
  console.log(`  installment ${fmt(installment)}; first due at ${l.firstDueAt} (from the contract, one interval after the opening)`);

  // Let the first installment go unpaid past its due date and the grace period.
  const target = l.firstDueAt + grace + 10n;
  if (target - (await chainNow(c)) > 600n) {
    console.log(`  the first installment falls due in ${intervalSeconds} s: run with --interval-seconds 20 on a local validator to see arrears`);
    return;
  }
  console.log(`  waiting until ${target} (due + ${grace} s grace + 10 s) without paying`);
  while ((await chainNow(c)) < target) await sleep(2_000);
  let a;
  ({ l, a } = await read());
  console.log(`  missed installments ${a.missed}; overdue ${fmt(a.overdue)}; late interest so far ${fmt(a.lateDue)}`);

  // The overdue installment, its late interest and the next installment; the program takes late
  // interest first, then interest, then principal, and never more than is due.
  const lateBefore = l.lateCollected;
  await repayInParts(c, { pool, poolData, line: sLine, payer: borrower, amount: a.overdue + installment + 2n * a.lateDue, receiptHash: noReceipt });
  ({ l, a } = await read());
  console.log(`  late interest collected ${fmt(l.lateCollected - lateBefore)}; missed installments now ${a.missed}`);
  console.log(`  repaid: interest ${fmt(l.interestRepaid)}, principal ${fmt(l.principalRepaid)} (late interest first, then interest, then principal)`);
  console.log(`  line status ${LineStatus[l.status]}; once 3 installments are missed and the grace period of the third has run, anyone may call mark_in_collection`);
}
