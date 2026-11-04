// The Squads v4 instructions the council tooling uses, hand-built on @solana/kit: no Squads SDK,
// which would bring @solana/web3.js v1 into a tool that signs for the council.
//
// Target: Squads v4 at af94153 (program 2.1.0), SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf; the
// binary scripts/local-e2e.sh preloads is programs/elote_pool/tests/fixtures/squads_v4-mainnet.so,
// sha256 dec8d3e0fae58c7c8f2416e5f67c25e673f047afd6dd2bba4a47e0b29a01d34c. The e2e executes every
// council step through it, so a wrong encoding fails there.
import { createHash } from "node:crypto";
import {
  type Address,
  type Instruction,
  type TransactionSigner,
  AccountRole,
  address,
  getAddressDecoder,
  getAddressEncoder,
  getProgramDerivedAddress,
} from "@solana/kit";

export const SQUADS_PROGRAM_ADDRESS = address("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");
export const SQUADS_FIXTURE_SHA256 = "dec8d3e0fae58c7c8f2416e5f67c25e673f047afd6dd2bba4a47e0b29a01d34c";
const SYSTEM_PROGRAM = address("11111111111111111111111111111111");

const disc = (namespace: string, name: string) => createHash("sha256").update(`${namespace}:${name}`).digest().subarray(0, 8);
const utf8 = (s: string) => new TextEncoder().encode(s);
const u8 = (n: number) => Uint8Array.of(n);
const u16 = (n: number) => {
  const b = new Uint8Array(2);
  new DataView(b.buffer).setUint16(0, n, true);
  return b;
};
const u32 = (n: number) => {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, n, true);
  return b;
};
const u64 = (n: bigint) => {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setBigUint64(0, n, true);
  return b;
};
const concat = (parts: Uint8Array[]) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
};
const key = (a: Address) => Uint8Array.from(getAddressEncoder().encode(a));

// -- PDAs -----------------------------------------------------------------------------------------

export async function multisigPda(createKey: Address): Promise<readonly [Address, number]> {
  return getProgramDerivedAddress({ programAddress: SQUADS_PROGRAM_ADDRESS, seeds: [utf8("multisig"), utf8("multisig"), key(createKey)] });
}

export async function vaultPda(multisig: Address, index = 0): Promise<readonly [Address, number]> {
  return getProgramDerivedAddress({ programAddress: SQUADS_PROGRAM_ADDRESS, seeds: [utf8("multisig"), key(multisig), utf8("vault"), u8(index)] });
}

export async function transactionPda(multisig: Address, index: bigint): Promise<Address> {
  const [pda] = await getProgramDerivedAddress({
    programAddress: SQUADS_PROGRAM_ADDRESS,
    seeds: [utf8("multisig"), key(multisig), utf8("transaction"), u64(index)],
  });
  return pda;
}

export async function proposalPda(multisig: Address, index: bigint): Promise<Address> {
  const [pda] = await getProgramDerivedAddress({
    programAddress: SQUADS_PROGRAM_ADDRESS,
    seeds: [utf8("multisig"), key(multisig), utf8("transaction"), u64(index), utf8("proposal")],
  });
  return pda;
}

// -- the Multisig account -------------------------------------------------------------------------

/** `Permissions` bits of a member. */
export const PERM_INITIATE = 1;
export const PERM_VOTE = 2;
export const PERM_EXECUTE = 4;

export type MultisigState = {
  createKey: Address;
  configAuthority: Address;
  threshold: number;
  timeLock: number;
  transactionIndex: bigint;
  staleTransactionIndex: bigint;
  bump: number;
  members: { key: Address; permissions: number }[];
};

/** Decodes a Squads v4 `Multisig` account (discriminator, then the fields in declaration order). */
export function decodeMultisig(data: Uint8Array): MultisigState {
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const dec = getAddressDecoder();
  if (Buffer.compare(Buffer.from(data.subarray(0, 8)), disc("account", "Multisig")) !== 0) throw new Error("not a Squads v4 Multisig account");
  let at = 8;
  const createKey = dec.decode(data.subarray(at, at + 32));
  at += 32;
  const configAuthority = dec.decode(data.subarray(at, at + 32));
  at += 32;
  const threshold = view.getUint16(at, true);
  const timeLock = view.getUint32(at + 2, true);
  const transactionIndex = view.getBigUint64(at + 6, true);
  const staleTransactionIndex = view.getBigUint64(at + 14, true);
  at += 22;
  at += data[at] === 1 ? 33 : 1; // rent_collector: Option<Pubkey>
  const bump = data[at];
  at += 1;
  const count = view.getUint32(at, true);
  at += 4;
  const members = [];
  for (let i = 0; i < count; i++, at += 33) members.push({ key: dec.decode(data.subarray(at, at + 32)), permissions: data[at + 32] });
  return { createKey, configAuthority, threshold, timeLock, transactionIndex, staleTransactionIndex, bump, members };
}

/**
 * The bytes of an autonomous Multisig account (no config authority, no time lock, no rent
 * collector), for a local validator's `--account` preload. Squads looks members up by binary
 * search, so they are sorted by key bytes.
 */
export function encodeMultisig(createKey: Address, bump: number, threshold: number, members: { key: Address; permissions: number }[]): Uint8Array {
  const sorted = [...members].sort((a, b) => Buffer.compare(Buffer.from(key(a.key)), Buffer.from(key(b.key))));
  return concat([
    disc("account", "Multisig"),
    key(createKey),
    new Uint8Array(32),
    u16(threshold),
    u32(0),
    u64(0n),
    u64(0n),
    u8(0),
    u8(bump),
    u32(sorted.length),
    ...sorted.map((m) => concat([key(m.key), u8(m.permissions)])),
  ]);
}

// -- the vault transaction message ----------------------------------------------------------------

type Meta = { address: Address; signer: boolean; writable: boolean };

const isSigner = (role: AccountRole) => role === AccountRole.READONLY_SIGNER || role === AccountRole.WRITABLE_SIGNER;
const isWritable = (role: AccountRole) => role === AccountRole.WRITABLE || role === AccountRole.WRITABLE_SIGNER;

/**
 * Compiles `instructions` into Squads' `TransactionMessage` (u8-length small vectors, data with a
 * u16 length, no lookup tables): the accounts merged by address, ordered writable signers,
 * readonly signers, writable non-signers, readonly non-signers. Returns the bytes and the account
 * keys in message order, which `vaultTransactionExecute` passes again.
 */
export function compileVaultMessage(instructions: Instruction[]): { bytes: Uint8Array; metas: Meta[] } {
  const merged = new Map<string, Meta>();
  const add = (a: Address, signer: boolean, writable: boolean) => {
    const m = merged.get(a);
    if (m) {
      m.signer ||= signer;
      m.writable ||= writable;
    } else merged.set(a, { address: a, signer, writable });
  };
  for (const ix of instructions) {
    for (const acc of ix.accounts ?? []) add(acc.address, isSigner(acc.role), isWritable(acc.role));
    add(ix.programAddress, false, false);
  }
  const rank = (m: Meta) => (m.signer ? (m.writable ? 0 : 1) : m.writable ? 2 : 3);
  const metas = [...merged.values()].sort((a, b) => rank(a) - rank(b));
  if (metas.length > 255) throw new Error("too many accounts for one vault transaction");
  const index = (a: Address) => metas.findIndex((m) => m.address === a);
  const compiled = instructions.map((ix) => {
    const data = ix.data ? Uint8Array.from(ix.data) : new Uint8Array();
    const accounts = (ix.accounts ?? []).map((acc) => index(acc.address));
    return concat([u8(index(ix.programAddress)), u8(accounts.length), Uint8Array.from(accounts), u16(data.length), data]);
  });
  const bytes = concat([
    u8(metas.filter((m) => m.signer).length),
    u8(metas.filter((m) => m.signer && m.writable).length),
    u8(metas.filter((m) => !m.signer && m.writable).length),
    u8(metas.length),
    ...metas.map((m) => key(m.address)),
    u8(compiled.length),
    ...compiled,
    u8(0),
  ]);
  return { bytes, metas };
}

// -- instructions ---------------------------------------------------------------------------------

const signerMeta = (s: TransactionSigner, writable: boolean) => ({ address: s.address, role: writable ? AccountRole.WRITABLE_SIGNER : AccountRole.READONLY_SIGNER, signer: s });
const meta = (a: Address, writable: boolean) => ({ address: a, role: writable ? AccountRole.WRITABLE : AccountRole.READONLY });

/** `vault_transaction_create` for vault 0, no ephemeral signers, no memo. */
export function vaultTransactionCreate(args: { multisig: Address; transaction: Address; creator: TransactionSigner; rentPayer: TransactionSigner; message: Uint8Array }): Instruction {
  return {
    programAddress: SQUADS_PROGRAM_ADDRESS,
    accounts: [meta(args.multisig, true), meta(args.transaction, true), signerMeta(args.creator, false), signerMeta(args.rentPayer, true), meta(SYSTEM_PROGRAM, false)],
    data: concat([disc("global", "vault_transaction_create"), u8(0), u8(0), u32(args.message.length), args.message, u8(0)]),
  };
}

/** `proposal_create`, active (not a draft). */
export function proposalCreate(args: { multisig: Address; proposal: Address; creator: TransactionSigner; rentPayer: TransactionSigner; transactionIndex: bigint }): Instruction {
  return {
    programAddress: SQUADS_PROGRAM_ADDRESS,
    accounts: [meta(args.multisig, false), meta(args.proposal, true), signerMeta(args.creator, false), signerMeta(args.rentPayer, true), meta(SYSTEM_PROGRAM, false)],
    data: concat([disc("global", "proposal_create"), u64(args.transactionIndex), u8(0)]),
  };
}

/** `proposal_approve` by `member`, no memo. */
export function proposalApprove(args: { multisig: Address; proposal: Address; member: TransactionSigner }): Instruction {
  return {
    programAddress: SQUADS_PROGRAM_ADDRESS,
    accounts: [meta(args.multisig, false), signerMeta(args.member, true), meta(args.proposal, true)],
    data: concat([disc("global", "proposal_approve"), u8(0)]),
  };
}

/**
 * `vault_transaction_execute` by `member`. The message's accounts follow in message order; the
 * vault is never a signer here (Squads signs for it), every other signer of the message must sign
 * the outer transaction: `signers` holds them by address.
 */
export function vaultTransactionExecute(args: {
  multisig: Address;
  proposal: Address;
  transaction: Address;
  member: TransactionSigner;
  vault: Address;
  metas: Meta[];
  signers: Map<string, TransactionSigner>;
}): Instruction {
  const outer = (m: Meta) => {
    if (!m.signer || m.address === args.vault) return meta(m.address, m.writable);
    const s = args.signers.get(m.address);
    if (!s) throw new Error(`the vault transaction needs ${m.address} to sign the execution`);
    return signerMeta(s, m.writable);
  };
  return {
    programAddress: SQUADS_PROGRAM_ADDRESS,
    accounts: [
      meta(args.multisig, false),
      meta(args.proposal, true),
      meta(args.transaction, false),
      signerMeta(args.member, false),
      ...args.metas.map(outer),
    ],
    data: disc("global", "vault_transaction_execute"),
  };
}
