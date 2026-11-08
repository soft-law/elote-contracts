// End-to-end demo of the elote_pool program: two investors deposit test USDC, the council (a Squads
// v4 multisig) publishes the FIX and approves a line in MXN, the borrower draws it as USDC at the
// FIX and repays it (principal by the borrower, interest by a sponsor), and the value of every
// share rises with the collected interest. With --schedule, a second line is repaid in six
// installments: one is missed and the next repayment collects late interest first.
//
//   npx tsx scripts/demo-pool.ts --cluster local|devnet --multisig <address> [--council-keypairs <a.json,b.json>]
//        [--asset-mint <address> [--scale <n>]] [--agreement-hash <64 hex>] [--sponsor]
//        [--schedule [--interval-seconds <n>]] [--max-line-bps <n>] [--yes]
//
// --asset-mint        an existing asset mint (a devnet build only accepts its allow-list: Circle's
//                     devnet USDC and the Etherfuse sandbox mints); the wallets are funded by
//                     transfer from the admin's balance instead of a fresh mint, so the admin must
//                     hold enough of it (the faucet)
// --fix               the FIX to publish, MXN per USD with up to four decimals (default 18.3740;
//                     1.0000 runs a peso series on a peso asset like MEXe: every conversion is 1:1)
// --scale             divide every demo amount by this (default 1, at most 1000, which fits a
//                     16-USDC faucet balance: deposits of 10 and 5, a line of MXN 60)
//                     NOTE: the pool address is ["pool", creator, asset_mint], so one admin key
//                     gets ONE pool per asset mint: a second --asset-mint run with the same admin
//                     fails at init_pool on the existing account
//
// --agreement-hash    hash of the signed credit agreement recorded on the line (placeholder if absent)
// --sponsor           the sponsor co-signs the opening and is named on the line
// --schedule          also open a 6-installment line, miss one installment and repay it with late interest
// --interval-seconds  seconds between installments (default: one month, 2,592,000; 20 on local, a
//                     compressed "demo month", since a validator's clock cannot be moved forward)
// --max-line-bps      largest line as a share of the NAV, in basis points (default 5000 here, so
//                     the MXN 60,000 line fits the 15,000 USDC of deposits; production pools use 1000)
// --multisig          the Squads v4 multisig whose vault 0 is the council; its members' keys come
//                     from --council-keypairs, ELOTE_COUNCIL_KEYPAIRS or .demo-keys/curator-<n>.json
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
  getTransferInstruction,
} from "@solana-program/token";
import {
  FX_REFERENCE_ACCOUNT,
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
  getPostFixInstructionAsync,
  getRequestLineInstructionAsync,
  getRedeemInstructionAsync,
  lineArrears,
  lineSchedule,
  mexicoCityDate,
  usdcCeilForMxn,
  usdcForMxn,
  parseFix,
} from "../clients/js/src/index.ts";
import {
  ASSET,
  DISCLAIMER,
  CENTAVOS,
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
  fmtMxn,
  integer,
  hash32,
  sendOrFail,
  sleep,
} from "./lib/cli.ts";
import { approveLine, councilExecute, loadCouncil } from "./lib/council.ts";
import { ataOf, repayInParts } from "./lib/pool.ts";

checkFlags(["cluster", "multisig", "council-keypairs", "asset-mint", "fix", "scale", "agreement-hash", "sponsor", "schedule", "interval-seconds", "max-line-bps", "yes"]);
const c = cluster();
const withSponsor = flag("sponsor");
const withSchedule = flag("schedule");
const MONTH_SECONDS = 30n * 86_400n;
const intervalSeconds = BigInt(arg("interval-seconds") ?? (c.local ? "20" : MONTH_SECONDS.toString()));
// The council: vault 0 of an existing autonomous Squads v4 multisig with a majority threshold.
const multisig = address(arg("multisig") ?? fail("--multisig <address> is required: the Squads v4 multisig whose vault 0 is the council"));
if (intervalSeconds <= 0n) fail("--interval-seconds must be positive");
// The concentration cap: production pools use 1,000 bps; the demo's MXN 60,000 line is about 22%
// of its 15,000 USDC of deposits at the FIX, so it defaults to 5,000.
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

// The demo's amounts, divided by --scale so a faucet-funded devnet run fits.
const scale = BigInt(integer("scale", 1, 1, 1_000)!);
const S = (n: bigint) => (n < scale ? 1n : n / scale);

// The test asset: an existing mint with --asset-mint (funded by transfer from the admin), or a
// fresh 6-decimal SPL mint per run, so every run gets its own pool.
const givenMint = arg("asset-mint") !== undefined ? address(arg("asset-mint")!) : undefined;
const mintSpace = BigInt(getMintSize());
const mintRent = await c.rpc.getMinimumBalanceForRentExemption(mintSpace).send();
let assetMintAddress: Address;
if (givenMint) {
  assetMintAddress = givenMint;
  console.log(`\nUsing the existing ${ASSET} mint ${assetMintAddress}`);
} else {
  console.log(`\nCreating the ${ASSET}`);
  const assetMint = await generateKeyPairSigner();
  await send(`create ${ASSET}`, admin, [
    getCreateAccountInstruction({ payer: admin, newAccount: assetMint, lamports: mintRent, space: mintSpace, programAddress: TOKEN_PROGRAM_ADDRESS }),
    getInitializeMint2Instruction({ mint: assetMint.address, decimals: 6, mintAuthority: admin.address }),
  ]);
  console.log(`  ${ASSET} address ${assetMint.address}`);
  assetMintAddress = assetMint.address;
}

const [pool] = await findPoolPda({ creator: admin.address, assetMint: assetMintAddress });

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
    assetMint: assetMintAddress,
    pool,
    shareMint,
    vault,
    deadShares,
    collection,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
    shareTokenProgram: TOKEN_PROGRAM_ADDRESS,
    maxLineBps,
    collectionPeriodSeconds: 30 * 86_400,
    maxOutstandingPrincipal: 2n ** 64n - 1n,
    accelerationNoticeSeconds: 7 * 86_400,
    // No FIX bounds in the demo; a production pool sets them (set_fix_bounds moves them).
    minFixRate: 0n,
    maxFixRate: 0n,
  }),
]);
console.log(
  `  pool ${pool}\n  share token ${shareMint}\n  collection ${collection}\n  vault ${vault}\n  max line ${maxLineBps} bps of the NAV`
);

// The council publishes the first FIX; draws and repayments are converted at it.
const council = await loadCouncil(c, multisig);
const FIX = arg("fix") !== undefined ? parseFix(arg("fix")!) : 183_740n; // MXN per USD; 18.3740 unless --fix
await councilExecute(c, council, `post_fix ${Number(FIX) / 10_000} for ${mexicoCityDate(await chainNow(c))}`, [
  await getPostFixInstructionAsync({
    poster: council.signer,
    multisig,
    pool,
    // RedStone's MXNe account; it does not exist on localnet or devnet, where the check is skipped.
    fxReference: FX_REFERENCE_ACCOUNT,
    fixRate: FIX,
    fixDate: mexicoCityDate(await chainNow(c)),
    validUntil: (await chainNow(c)) + 86_400n,
  }),
]).catch((e) => fail((e as Error).message));

console.log(`\nIssuing ${ASSET} to the investors and the sponsor`);
const issue: Instruction[] = [];
const allocations = [
  [investorA, S(10_000n) * UNIT],
  [investorB, S(5_000n) * UNIT],
  [sponsor, S(720n * UNIT)],
] as const;
for (const [w, amount] of allocations) {
  issue.push(await getCreateAssociatedTokenIdempotentInstructionAsync({ payer: admin, owner: w.address, mint: assetMintAddress }));
  if (givenMint) {
    const total = allocations.reduce((a, [, n]) => a + n, 0n);
    const held = (await fetchToken(c.rpc, await ataOf(admin.address, assetMintAddress), { commitment: "confirmed" }).catch(() => fail(`the admin holds no ${ASSET}: fund ${admin.address} from the faucet`)))!.data.amount;
    if (held < total) fail(`the admin holds ${fmt(held)} ${ASSET}, the demo needs ${fmt(total)}: fund ${admin.address} or raise --scale`);
    issue.push(getTransferInstruction({ source: await ataOf(admin.address, assetMintAddress), destination: await ataOf(w.address, assetMintAddress), authority: admin, amount }));
  } else {
    issue.push(getMintToInstruction({ mint: assetMintAddress, token: await ataOf(w.address, assetMintAddress), mintAuthority: admin, amount }));
  }
}
issue.push(await getCreateAssociatedTokenIdempotentInstructionAsync({ payer: admin, owner: borrower.address, mint: assetMintAddress }));
await send(`issue ${ASSET}: A ${fmt(S(10_000n) * UNIT)}, B ${fmt(S(5_000n) * UNIT)}, sponsor ${fmt(S(720n * UNIT))}`, admin, issue);

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
      assetMint: assetMintAddress,
      shareMint,
      vault,
      tokenProgram: TOKEN_PROGRAM_ADDRESS,
      shareTokenProgram: TOKEN_PROGRAM_ADDRESS,
      depositorAssetAccount: await ataOf(who.address, assetMintAddress),
      depositorShareAccount: await ataOf(who.address, shareMint),
      deadShares,
      assets,
      minSharesOut,
    }),
  ]);
}

console.log("\nDeposits");
await deposit(`investor A deposits ${fmt(S(10_000n) * UNIT)} ${ASSET}`, investorA, S(10_000n) * UNIT);
await deposit(`investor B deposits ${fmt(S(5_000n) * UNIT)} ${ASSET}`, investorB, S(5_000n) * UNIT);
const before = await snapshot();

console.log(`\nCredit line: MXN ${fmtMxn(S(60_000n) * CENTAVOS)} at 12%${withSponsor ? ", co-signed by the sponsor" : ""}, drawn as USDC at the FIX`);
const { lineCount } = (await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data;
const [line] = await findLinePda({ pool, lineId: lineCount });
const termEnd = BigInt(Math.floor(Date.now() / 1000) + 90 * 86_400);
await send("request_line", admin, [
  await getRequestLineInstructionAsync({
    borrower,
    payer: admin,
    ...(withSponsor ? { sponsor } : {}),
    pool,
    line,
    lineId: lineCount,
    principal: S(60_000n) * CENTAVOS,
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
await approveLine(c, council, pool, lineCount).catch((e) => fail((e as Error).message));
const borrowerAsset = await ataOf(borrower.address, assetMintAddress);

/** A draw: the borrower receives `amount`; the admin pays the pagaré's rent as the app's relayer. */
async function drawIx(lineAddress: Address, drawIndex: number, amount: bigint) {
  return getDrawInstructionAsync({
    borrower,
    pool,
    line: lineAddress,
    assetMint: assetMintAddress,
    vault,
    borrowerAssetAccount: borrowerAsset,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
    payer: admin,
    ...(await drawPagareAccounts({ line: lineAddress, drawIndex, collection })),
    systemProgram: SYSTEM_PROGRAM_ADDRESS,
    amountMxn: amount,
    fixRate: (await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data.fixRate,
    fixDate: (await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data.fixDate,
    certFingerprint: new Uint8Array(32), // wallet-only signature in the demo: no certificate
    signatureMethod: 0,
  });
}

await send(`draw: MXN ${fmtMxn(S(60_000n) * CENTAVOS)} as ${fmt(usdcForMxn(S(60_000n) * CENTAVOS, FIX))} ${ASSET} to the borrower, pagaré 0 minted`, borrower, [
  await drawIx(line, 0, S(60_000n) * CENTAVOS),
]);
const drawn = await snapshot();
const poolData = (await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data;
const noReceipt = new Uint8Array(32); // the demo has no receipt: "to follow"
console.log(`\nRepayments (interest is taken first)`);
const borrowerHolds = (await fetchToken(c.rpc, borrowerAsset, { commitment: "confirmed" })).data.amount;
await repayInParts(c, { pool, poolData, line, payer: borrower, amount: borrowerHolds, receiptHash: noReceipt });
await repayInParts(c, { pool, poolData, line, payer: sponsor, amount: S(720n * UNIT), receiptHash: noReceipt });
const repaid = await snapshot();
const status = (await fetchCreditLine(c.rpc, line, { commitment: "confirmed" })).data.status;

console.log("\nRedemption");
const redeemShares = S(4_000n) * UNIT;
await send(`investor A redeems ${fmt(redeemShares)} shares`, investorA, [
  await getRedeemInstructionAsync({
    holder: investorA,
    pool,
    assetMint: assetMintAddress,
    shareMint,
    vault,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
    shareTokenProgram: TOKEN_PROGRAM_ADDRESS,
    holderShareAccount: await ataOf(investorA.address, shareMint),
    holderAssetAccount: await ataOf(investorA.address, assetMintAddress),
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
  const principal = S(6_000n) * CENTAVOS;
  const label = intervalSeconds === MONTH_SECONDS ? "monthly" : `every ${intervalSeconds} s (a compressed demo month)`;
  console.log(`\nInstallment line: MXN ${fmtMxn(principal)} at 12%, ${installments} installments ${label}, late interest 100% a year, collection after 3 missed`);
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
  await approveLine(c, council, pool, id).catch((e) => fail((e as Error).message));
  await send(`draw: MXN ${fmtMxn(principal)} to the borrower, pagaré 0 minted`, borrower, [await drawIx(sLine, 0, principal)]);

  const read = async () => {
    const l = (await fetchCreditLine(c.rpc, sLine, { commitment: "confirmed" })).data;
    const s = lineSchedule(l)!;
    const now = await chainNow(c);
    return { l, s, now, a: lineArrears(l, s, 0n, now) };
  };
  let { l } = await read();
  const installment = (l.drawn + l.interestDue) / BigInt(installments);
  console.log(`  installment MXN ${fmtMxn(installment)}; first due at ${l.firstDueAt} (from the contract, one interval after the opening)`);

  // Let the first installment go unpaid past its due date: late interest runs from then on.
  const target = l.firstDueAt + 10n;
  if (target - (await chainNow(c)) > 600n) {
    console.log(`  the first installment falls due in ${intervalSeconds} s: run with --interval-seconds 20 on a local validator to see arrears`);
    return;
  }
  console.log(`  waiting until ${target} (due + 10 s) without paying`);
  while ((await chainNow(c)) < target) await sleep(2_000);
  let a;
  ({ l, a } = await read());
  console.log(`  missed installments ${a.missed}; overdue MXN ${fmtMxn(a.overdue)}; late interest so far MXN ${fmtMxn(a.lateDue)}`);

  // The overdue installment, its late interest and the next installment; the program takes late
  // interest first, then interest, then principal, and never more than is due.
  const lateBefore = l.lateCollected;
  await repayInParts(c, { pool, poolData, line: sLine, payer: borrower, amount: usdcCeilForMxn(a.overdue + installment + 2n * a.lateDue, FIX), receiptHash: noReceipt });
  ({ l, a } = await read());
  console.log(`  late interest collected MXN ${fmtMxn(l.lateCollected - lateBefore)}; missed installments now ${a.missed}`);
  console.log(`  repaid: interest MXN ${fmtMxn(l.interestRepaid)}, principal MXN ${fmtMxn(l.principalRepaid)} (late interest first, then interest, then principal)`);
  console.log(`  line status ${LineStatus[l.status]}; once 3 installments are missed, past the third one's due date, anyone may call mark_in_collection`);
}
