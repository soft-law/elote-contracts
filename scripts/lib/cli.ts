// What every script shares: arguments, the cluster and its RPC, the confirmation before sending
// anywhere but this machine, keys, and one send path that decodes elote_pool's errors through the
// generated client.
import { createHash, generateKeyPairSync } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import readline from "node:readline/promises";
import {
  type Address,
  type Instruction,
  type KeyPairSigner,
  type Rpc,
  type SolanaRpcApi,
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
import {
  type ElotePoolError,
  getElotePoolErrorMessage,
  isElotePoolError,
} from "../../clients/js/src/index.ts";

export const DISCLAIMER = "Demo on Solana devnet with test assets. Not an offer of securities.";
export const ASSET = "tMXN (devnet test token, no value)";
export const DECIMALS = 6;
export const UNIT = 10n ** BigInt(DECIMALS);
export const DAY = 86_400n;
export const KEY_DIR = path.resolve(".demo-keys");

export function fail(message: string): never {
  console.error(`error: ${message}`);
  process.exit(1);
}

// -- arguments ------------------------------------------------------------------------------------

const argv = process.argv.slice(2);

/** `--name <value>`; undefined when absent. A flag given without a value is an error. */
export function arg(name: string): string | undefined {
  const i = argv.indexOf(`--${name}`);
  if (i < 0) return undefined;
  const value = argv[i + 1];
  if (value === undefined || value.startsWith("--")) fail(`--${name} needs a value`);
  return value;
}

/** `--name` with no value. */
export function flag(name: string): boolean {
  return argv.includes(`--${name}`);
}

export function required(name: string): string {
  return arg(name) ?? fail(`--${name} is required`);
}

/** Refuses any `--flag` not in `known`, so a typo never silently falls back to a default. */
export function checkFlags(known: readonly string[]): void {
  for (const a of argv) {
    if (a.startsWith("--") && !known.includes(a.slice(2))) fail(`unknown flag ${a}`);
  }
}

export function integer(name: string, fallback: number | undefined, min: number, max: number): number | undefined {
  const raw = arg(name);
  if (raw === undefined) return fallback;
  const n = Number(raw);
  if (!Number.isInteger(n) || n < min || n > max) fail(`--${name} must be an integer from ${min} to ${max}`);
  return n;
}

/** An amount of the asset with up to `DECIMALS` decimals ("500", "1250.5"), in base units. */
export function parseAmount(name: string, raw: string): bigint {
  if (!/^\d+(\.\d+)?$/.test(raw)) fail(`--${name} must be a positive amount, like 500 or 1250.5`);
  const [whole, frac = ""] = raw.split(".");
  if (frac.length > DECIMALS) fail(`--${name} has more than ${DECIMALS} decimals`);
  const units = BigInt(whole) * UNIT + BigInt(frac.padEnd(DECIMALS, "0"));
  if (units === 0n) fail(`--${name} must be more than zero`);
  return units;
}

/** 32 bytes of hex from `--name`, or the sha256 of `placeholder` when absent (and says so). */
export function hash32(name: string, placeholder?: string): Uint8Array {
  const given = arg(name);
  if (given !== undefined) {
    const hex = given.replace(/^0x/, "");
    if (!/^[0-9a-fA-F]{64}$/.test(hex)) fail(`--${name} must be 32 bytes of hex`);
    return Uint8Array.from(Buffer.from(hex, "hex"));
  }
  if (placeholder === undefined) fail(`--${name} is required`);
  console.log(`No --${name} given: using the sha256 of "${placeholder}" as a placeholder.`);
  return Uint8Array.from(createHash("sha256").update(placeholder).digest());
}

// -- cluster --------------------------------------------------------------------------------------

export type Cluster = { name: "local" | "devnet"; rpcUrl: string; local: boolean; rpc: Rpc<SolanaRpcApi> };

/** True when `url` points at this machine. */
export function isLocalhost(url: string): boolean {
  try {
    const host = new URL(url).hostname;
    return host === "localhost" || host === "127.0.0.1" || host === "[::1]" || host === "::1";
  } catch {
    return false;
  }
}

/** `--cluster local|devnet`; `ELOTE_RPC` overrides the RPC URL. */
export function cluster(): Cluster {
  const name = required("cluster");
  if (name !== "local" && name !== "devnet") fail("--cluster must be local or devnet");
  const rpcUrl = process.env.ELOTE_RPC ?? (name === "devnet" ? "https://api.devnet.solana.com" : "http://127.0.0.1:8899");
  const local = isLocalhost(rpcUrl);
  if (name === "local" && !local) fail(`--cluster local but ELOTE_RPC points at ${rpcUrl}`);
  return { name, rpcUrl, local, rpc: createSolanaRpc(rpcUrl) as Rpc<SolanaRpcApi> };
}

/**
 * Prints where the transactions go and, unless that is this machine or `--yes` was given, asks
 * before sending anything.
 */
export async function confirmCluster(c: Cluster, what: string): Promise<void> {
  console.log(`${DISCLAIMER}\nCluster: ${c.name} (${c.rpcUrl})\n`);
  if (c.local || flag("yes")) return;
  if (!process.stdin.isTTY) fail(`refusing to send to ${c.rpcUrl} without a terminal; pass --yes to confirm`);
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  const answer = (await rl.question(`Send ${what} to ${c.rpcUrl}? Type "yes" to continue: `)).trim();
  rl.close();
  if (answer !== "yes") fail("not confirmed; nothing was sent");
}

// -- keys -----------------------------------------------------------------------------------------

/** Loads an existing 64-byte Solana keypair file. Never printed, never created here. */
export async function loadSigner(label: string, file: string): Promise<KeyPairSigner> {
  const resolved = path.resolve(file.replace(/^~/, os.homedir()));
  if (!fs.existsSync(resolved)) fail(`${label} keypair file not found (${file})`);
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

/** The key in `envVar`, else `.demo-keys/<name>.json`, created there on first use. Never printed. */
export async function demoSigner(name: string, envVar: string): Promise<KeyPairSigner> {
  const fromEnv = process.env[envVar];
  if (fromEnv) return loadSigner(name, fromEnv);
  const file = path.join(KEY_DIR, `${name}.json`);
  if (!fs.existsSync(file)) {
    fs.mkdirSync(KEY_DIR, { recursive: true, mode: 0o700 });
    const { privateKey, publicKey } = generateKeyPairSync("ed25519");
    const seed = Buffer.from(privateKey.export({ format: "jwk" }).d!, "base64url");
    const pub = Buffer.from(publicKey.export({ format: "jwk" }).x!, "base64url");
    fs.writeFileSync(file, JSON.stringify([...seed, ...pub]), { mode: 0o600 });
  }
  return loadSigner(name, file);
}

/** `--<name>-keypair <path>`, else the `ELOTE_<NAME>_KEYPAIR` variable, else `.demo-keys/<name>.json`. */
export async function roleSigner(name: string): Promise<KeyPairSigner> {
  const envVar = `ELOTE_${name.toUpperCase().replace(/-/g, "_")}_KEYPAIR`;
  const file = arg(`${name}-keypair`) ?? process.env[envVar] ?? path.join(KEY_DIR, `${name}.json`);
  return loadSigner(name, file);
}

// -- transactions ---------------------------------------------------------------------------------

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function explorer(c: Cluster, signature: string): string {
  const q = c.name === "devnet" && !process.env.ELOTE_RPC ? "devnet" : `custom&customUrl=${encodeURIComponent(c.rpcUrl)}`;
  return `https://explorer.solana.com/tx/${signature}?cluster=${q}`;
}

/** The program's error in a failed send, named through the generated client; null otherwise. */
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

/** Same for the JSON `err` of a signature status. */
function statusError(err: unknown): string {
  const custom = (err as { InstructionError?: [number, { Custom?: number }] })?.InstructionError?.[1]?.Custom;
  if (typeof custom === "number") {
    const name = getElotePoolErrorMessage(custom as ElotePoolError);
    if (name) return `program error ${custom}: ${name}`;
  }
  return JSON.stringify(err, (_, v) => (typeof v === "bigint" ? v.toString() : v));
}

export class SendError extends Error {}

/**
 * Signs, sends and waits for `confirmed`. Throws `SendError` with the program's error decoded;
 * prints the label, the signature and an explorer link on success.
 */
export async function send(c: Cluster, label: string, feePayer: KeyPairSigner, instructions: Instruction[]): Promise<string> {
  const { value: blockhash } = await c.rpc.getLatestBlockhash({ commitment: "confirmed" }).send();
  const message = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayerSigner(feePayer, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
    (m) => appendTransactionMessageInstructions(instructions, m)
  );
  const tx = await signTransactionMessageWithSigners(message);
  const signature = getSignatureFromTransaction(tx);
  try {
    await c.rpc
      .sendTransaction(getBase64EncodedWireTransaction(tx), { encoding: "base64", preflightCommitment: "confirmed" })
      .send();
  } catch (error) {
    throw new SendError(`${label} was rejected: ${programError(error, message) ?? (error as Error).message}`);
  }
  for (let i = 0; i < 60; i++) {
    await sleep(1_000);
    const {
      value: [status],
    } = await c.rpc.getSignatureStatuses([signature]).send();
    if (status?.err) throw new SendError(`${label} failed (${signature}): ${statusError(status.err)}`);
    if (status?.confirmationStatus === "confirmed" || status?.confirmationStatus === "finalized") {
      console.log(`  ${label}\n    ${signature}\n    ${explorer(c, signature)}`);
      return signature;
    }
  }
  throw new SendError(`${label}: not confirmed in time (${signature}); check the explorer before retrying`);
}

/** `send`, exiting with the decoded error instead of throwing. */
export async function sendOrFail(c: Cluster, label: string, feePayer: KeyPairSigner, instructions: Instruction[]): Promise<string> {
  try {
    return await send(c, label, feePayer, instructions);
  } catch (e) {
    if (e instanceof SendError) fail(e.message);
    throw e;
  }
}

/** The cluster's `Clock::unix_timestamp`: what the program reads as `now`. */
export async function chainNow(c: Cluster): Promise<bigint> {
  const clock = "SysvarC1ock11111111111111111111111111111111" as Address;
  const { value } = await c.rpc.getAccountInfo(clock, { encoding: "base64", commitment: "confirmed" }).send();
  return Buffer.from(value!.data[0], "base64").readBigInt64LE(32);
}

export const fmt = (units: bigint) => {
  const whole = units / UNIT;
  const frac = (units % UNIT).toString().padStart(DECIMALS, "0").replace(/0+$/, "");
  return `${whole.toLocaleString("en-US")}${frac ? "." + frac : ""}`;
};
