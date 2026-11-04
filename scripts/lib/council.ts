// The council through Squads: load the multisig and its members' keys, then run a decision as one
// vault transaction (create, propose, approve by a threshold of members, execute). The vault signs
// only inside Squads' execution; the other signers of the decision sign the execution.
import fs from "node:fs";
import path from "node:path";
import { type Address, type Instruction, type KeyPairSigner, type TransactionSigner, createNoopSigner } from "@solana/kit";
import { LineStatus, getApproveLineInstructionAsync, termsHash } from "../../clients/js/src/index.ts";
import { type Cluster, KEY_DIR, arg, chainNow, fail, loadSigner, send } from "./cli.ts";
import { loadLine, loadPool } from "./pool.ts";
import {
  PERM_EXECUTE,
  PERM_INITIATE,
  PERM_VOTE,
  SQUADS_PROGRAM_ADDRESS,
  compileVaultMessage,
  decodeMultisig,
  proposalApprove,
  proposalCreate,
  proposalPda,
  transactionPda,
  vaultPda,
  vaultTransactionCreate,
  vaultTransactionExecute,
} from "./squads.ts";

const COMPUTE_BUDGET = "ComputeBudget111111111111111111111111111111" as Address;
/** The compute rule: 100,000 CU per transaction, the Squads execution included. */
export const COUNCIL_CU_LIMIT = 100_000;

export type Council = {
  multisig: Address;
  vault: Address;
  /** Stands in for the vault in instruction builders: it never signs, Squads does. */
  signer: TransactionSigner;
  threshold: number;
  members: KeyPairSigner[];
};

/**
 * The keys of the members who act: `--council-keypairs <a.json,b.json,...>`, else
 * `ELOTE_COUNCIL_KEYPAIRS`, else `.demo-keys/curator-1.json` … `curator-3.json` that exist. Never
 * printed.
 */
async function memberKeys(): Promise<KeyPairSigner[]> {
  const list = arg("council-keypairs") ?? process.env.ELOTE_COUNCIL_KEYPAIRS;
  const files = list
    ? list.split(",").map((f) => f.trim()).filter(Boolean)
    : [1, 2, 3].map((i) => path.join(KEY_DIR, `curator-${i}.json`)).filter((f) => fs.existsSync(f));
  return Promise.all(files.map((f, i) => loadSigner(`curator ${i + 1}`, f)));
}

export async function loadCouncil(c: Cluster, multisig: Address): Promise<Council> {
  const { value } = await c.rpc.getAccountInfo(multisig, { encoding: "base64", commitment: "confirmed" }).send();
  if (!value) fail(`no account at ${multisig}`);
  if (value.owner !== SQUADS_PROGRAM_ADDRESS) fail(`${multisig} is not a Squads v4 account`);
  const state = decodeMultisig(Uint8Array.from(Buffer.from(value.data[0], "base64")));
  if (state.timeLock !== 0) fail("the multisig has a time lock: proposals cannot execute at once");
  const keys = await memberKeys();
  const members = keys.filter((k) => state.members.some((m) => m.key === k.address && (m.permissions & PERM_VOTE) !== 0));
  if (members.length < state.threshold) {
    fail(`the council needs ${state.threshold} voting members' keys; ${members.length} given (--council-keypairs or ELOTE_COUNCIL_KEYPAIRS)`);
  }
  const first = state.members.find((m) => m.key === members[0].address)!;
  if ((first.permissions & PERM_INITIATE) === 0 || (first.permissions & PERM_EXECUTE) === 0) {
    fail(`${members[0].address} cannot initiate and execute: put a member with every permission first`);
  }
  const [vault] = await vaultPda(multisig);
  return { multisig, vault, signer: createNoopSigner(vault), threshold: state.threshold, members: members.slice(0, state.threshold) };
}

/**
 * Runs `instructions` as one decision of the council: the first member creates and proposes the
 * vault transaction and pays its rent, the threshold of members approves, and the first member
 * executes, with a 100,000 CU limit on the execution. `extraSigners` are the decision's own signers
 * other than the vault (for example a rent payer). Returns the execution's signature.
 */
export async function councilExecute(
  c: Cluster,
  council: Council,
  label: string,
  instructions: Instruction[],
  extraSigners: TransactionSigner[] = []
): Promise<string> {
  const [creator, ...approvers] = council.members;
  const { value } = await c.rpc.getAccountInfo(council.multisig, { encoding: "base64", commitment: "confirmed" }).send();
  const index = decodeMultisig(Uint8Array.from(Buffer.from(value!.data[0], "base64"))).transactionIndex + 1n;
  const transaction = await transactionPda(council.multisig, index);
  const proposal = await proposalPda(council.multisig, index);
  const { bytes, metas } = compileVaultMessage(instructions);
  console.log(`  council: ${label} as vault transaction ${index} of ${council.multisig}`);
  await send(c, `propose and approve ${label} (${council.threshold} of the council)`, creator, [
    vaultTransactionCreate({ multisig: council.multisig, transaction, creator, rentPayer: creator, message: bytes }),
    proposalCreate({ multisig: council.multisig, proposal, creator, rentPayer: creator, transactionIndex: index }),
    proposalApprove({ multisig: council.multisig, proposal, member: creator }),
    ...approvers.map((member) => proposalApprove({ multisig: council.multisig, proposal, member })),
  ]);
  const signers = new Map<string, TransactionSigner>([[creator.address, creator], ...extraSigners.map((s) => [s.address, s] as [string, TransactionSigner])]);
  const limit = new Uint8Array(5);
  limit[0] = 2; // SetComputeUnitLimit
  new DataView(limit.buffer).setUint32(1, COUNCIL_CU_LIMIT, true);
  return send(c, `execute ${label}`, creator, [
    { programAddress: COMPUTE_BUDGET, accounts: [], data: limit },
    vaultTransactionExecute({ multisig: council.multisig, proposal, transaction, member: creator, vault: council.vault, metas, signers }),
  ]);
}

/**
 * `approve_line` by the council for line `lineId`: the terms hash is recomputed from the stored
 * terms with the client's `termsHash` and must equal the one the program stored (the curators'
 * tooling computes it from the documents instead). The decision expires a day after it is built.
 */
export async function approveLine(c: Cluster, council: Council, pool: Address, lineId: bigint): Promise<string> {
  const poolData = await loadPool(c, pool);
  const { line, data: l } = await loadLine(c, pool, lineId);
  if (l.status !== LineStatus.Requested) fail(`line ${lineId} is ${LineStatus[l.status]}, not Requested`);
  const computed = await termsHash(
    pool,
    lineId,
    l.borrower,
    {
      principal: l.principal,
      rateBps: l.rateBps,
      lateBps: l.lateBps,
      termEnd: l.termEnd,
      agreementHash: l.agreementHash,
      sponsor: l.sponsor.__option === "Some" ? l.sponsor.value : null,
      sponsorRole: l.sponsorRole,
      schedule:
        l.installments > 0
          ? { firstDueAt: l.firstDueAt, intervalSeconds: l.intervalSeconds, installments: l.installments, maxMissed: l.maxMissed }
          : null,
      expedienteRoot: l.expedienteRoot,
      pagareTemplateHash: l.pagareTemplateHash,
    },
    l.currency
  );
  if (Buffer.compare(Buffer.from(computed), Buffer.from(l.termsHash)) !== 0) {
    fail("the terms hash recomputed from the line is not the one the program stored: check the client's encoding");
  }
  console.log(`  line ${lineId}: terms hash ${Buffer.from(computed).toString("hex")}`);
  return councilExecute(c, council, `approve_line ${lineId}`, [
    await getApproveLineInstructionAsync({
      council: council.signer,
      multisig: poolData.multisig,
      pool,
      line,
      vault: poolData.vault,
      expedienteRoot: l.expedienteRoot,
      termsHash: computed,
      validUntil: (await chainNow(c)) + 86_400n,
    }),
  ]);
}
