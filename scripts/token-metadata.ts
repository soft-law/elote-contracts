// Creates the Metaplex Token Metadata account of an existing SPL token, so wallets show its name,
// symbol and icon (tMXN, the devnet test token, instead of "Unknown token").
//
//   npx tsx scripts/token-metadata.ts --cluster devnet|local --mint <address>
//        [--authority-keypair <path>] [--update-authority <address>] [--uri <https url>]
//        [--name "tMXN test token"] [--symbol tMXN] [--skip-uri-check] [--dry-run]
//
// --authority-keypair  keypair file of the token's mint authority, which signs and pays
//                      (default: env ELOTE_TOKEN_AUTHORITY_KEYPAIR)
// --update-authority   address allowed to change the metadata later (default: the demo admin of
//                      .demo-keys/admin.json if present, else the signer); it does not sign
// --uri                off-chain JSON (default https://elote.baby/token/tmxn.json); checked with a
//                      request before sending unless --skip-uri-check
// --dry-run            prints the instruction's accounts and sends nothing; without a keypair it
//                      uses the token's on-chain mint authority as a placeholder signer
//
// Uses createMetadataAccountV3 from @metaplex-foundation/mpl-token-metadata 3.4
// (dist/src/generated/instructions/createMetadataAccountV3.d.ts): mutable, no seller fee, no
// creators, no collection. Refuses if the metadata account already exists. Only addresses are
// printed; ELOTE_RPC overrides the cluster's RPC URL.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { address, createSolanaRpc } from "@solana/kit";
import { fetchMaybeMint } from "@solana-program/token";
import { createMetadataAccountV3, findMetadataPda, mplTokenMetadata, safeFetchMetadata } from "@metaplex-foundation/mpl-token-metadata";
import { createNoopSigner, createSignerFromKeypair, none, publicKey, signerIdentity, type PublicKey, type Signer } from "@metaplex-foundation/umi";
import { createUmi } from "@metaplex-foundation/umi-bundle-defaults";
import { base58 } from "@metaplex-foundation/umi/serializers";

const DISCLAIMER = "Demo on Solana devnet with test assets. Not an offer of securities.";
const DEFAULT_URI = "https://elote.baby/token/tmxn.json";
// Limits of the Token Metadata program (MAX_NAME_LENGTH, MAX_SYMBOL_LENGTH, MAX_URI_LENGTH).
const MAX_NAME = 32;
const MAX_SYMBOL = 10;
const MAX_URI = 200;

function fail(message: string): never {
  console.error(`token-metadata: ${message}`);
  process.exit(1);
}

// -- arguments ------------------------------------------------------------------------------------

const VALUE_FLAGS = ["cluster", "mint", "uri", "name", "symbol", "authority-keypair", "update-authority"];
const BOOL_FLAGS = ["dry-run", "skip-uri-check"];
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) {
  const name = argv[i].startsWith("--") ? argv[i].slice(2) : "";
  if (BOOL_FLAGS.includes(name)) continue;
  if (!VALUE_FLAGS.includes(name)) {
    fail(`unknown argument ${JSON.stringify(argv[i])}; flags: --${[...VALUE_FLAGS, ...BOOL_FLAGS].join(", --")}`);
  }
  i++;
}

function arg(name: string): string | undefined {
  const i = argv.indexOf(`--${name}`);
  if (i < 0) return undefined;
  const value = argv[i + 1];
  if (value === undefined || value.startsWith("--")) fail(`--${name} needs a value`);
  return value;
}
const flag = (name: string) => argv.includes(`--${name}`);

function asAddress(label: string, value: string): string {
  try {
    return address(value);
  } catch {
    fail(`${label} is not a valid address`);
  }
}

const cluster = arg("cluster") ?? fail("--cluster is required (devnet or local)");
if (cluster !== "local" && cluster !== "devnet") fail("--cluster must be devnet or local");
const mintAddress = asAddress("--mint", arg("mint") ?? fail("--mint is required"));
const uri = arg("uri") ?? DEFAULT_URI;
const name = arg("name") ?? "tMXN test token";
const symbol = arg("symbol") ?? "tMXN";
const authorityFile = arg("authority-keypair") ?? process.env.ELOTE_TOKEN_AUTHORITY_KEYPAIR;
const updateAuthorityArg = arg("update-authority");
const dryRun = flag("dry-run");
const skipUriCheck = flag("skip-uri-check");

if (!/^https:\/\//.test(uri)) fail("--uri must be an https URL");
const bytes = (s: string) => Buffer.byteLength(s, "utf8");
if (bytes(name) === 0 || bytes(name) > MAX_NAME) fail(`--name must be 1 to ${MAX_NAME} bytes`);
if (bytes(symbol) === 0 || bytes(symbol) > MAX_SYMBOL) fail(`--symbol must be 1 to ${MAX_SYMBOL} bytes`);
if (bytes(uri) > MAX_URI) fail(`--uri must be at most ${MAX_URI} bytes`);
if (!authorityFile && !dryRun) fail("--authority-keypair (or ELOTE_TOKEN_AUTHORITY_KEYPAIR) is required");

const rpcUrl = process.env.ELOTE_RPC ?? (cluster === "devnet" ? "https://api.devnet.solana.com" : "http://127.0.0.1:8899");
const umi = createUmi(rpcUrl, "confirmed").use(mplTokenMetadata());
const rpc = createSolanaRpc(rpcUrl);

// -- keys -----------------------------------------------------------------------------------------

/** Reads a 64-byte Solana keypair file. The secret is never printed. */
function readKeypairBytes(label: string, file: string): Uint8Array {
  const resolved = path.resolve(file.replace(/^~/, os.homedir()));
  if (!fs.existsSync(resolved)) fail(`${label} keypair file not found`);
  let data: Uint8Array;
  try {
    data = Uint8Array.from(JSON.parse(fs.readFileSync(resolved, "utf8")));
  } catch {
    fail(`${label} keypair file is not a JSON array of bytes`);
  }
  if (data.length !== 64) fail(`${label} keypair file must hold 64 bytes`);
  return data;
}

function loadSigner(label: string, file: string): Signer {
  try {
    return createSignerFromKeypair(umi, umi.eddsa.createKeypairFromSecretKey(readKeypairBytes(label, file)));
  } catch {
    fail(`${label} keypair file does not hold a valid keypair`);
  }
}

/** The demo admin's address, from .demo-keys/admin.json, or null if the file is absent. */
function demoAdmin(): PublicKey | null {
  const file = path.join(".demo-keys", "admin.json");
  if (!fs.existsSync(file)) return null;
  return umi.eddsa.createKeypairFromSecretKey(readKeypairBytes("admin", file)).publicKey;
}

// -- checks ---------------------------------------------------------------------------------------

/** The URI must answer 200 with a JSON body whose name and symbol match the on-chain fields. */
async function checkUri() {
  let response: Response;
  try {
    response = await fetch(uri, { redirect: "follow", headers: { accept: "application/json" } });
  } catch (error) {
    fail(`could not fetch ${uri}: ${(error as Error).message}. Publish the JSON first, or pass --skip-uri-check`);
  }
  if (response.status !== 200) fail(`${uri} answered ${response.status}, not 200. Publish the JSON first, or pass --skip-uri-check`);
  const type = response.headers.get("content-type") ?? "";
  if (!type.includes("application/json")) fail(`${uri} is served as ${JSON.stringify(type)}, not application/json`);
  let body: { name?: unknown; symbol?: unknown; image?: unknown };
  try {
    body = (await response.json()) as typeof body;
  } catch {
    fail(`${uri} is not valid JSON`);
  }
  if (body.name !== name || body.symbol !== symbol) {
    fail(`${uri} has name ${JSON.stringify(body.name)} and symbol ${JSON.stringify(body.symbol)}; the transaction would write ${JSON.stringify(name)} and ${JSON.stringify(symbol)}`);
  }
  if (typeof body.image !== "string") fail(`${uri} has no image URL`);
  console.log(`URI checked: 200 JSON, image ${body.image}`);
}

function explorer(kind: "tx" | "address", value: string): string {
  const c = cluster === "devnet" ? "devnet" : `custom&customUrl=${encodeURIComponent(rpcUrl)}`;
  return `https://explorer.solana.com/${kind}/${value}?cluster=${c}`;
}

// -- create the metadata --------------------------------------------------------------------------

async function main() {
  console.log(`${DISCLAIMER}\nCluster: ${cluster} (${rpcUrl})${dryRun ? ", dry run: nothing is sent" : ""}\n`);

  const token = await fetchMaybeMint(rpc, address(mintAddress), { commitment: "confirmed" });
  if (!token.exists) fail(`no SPL token at ${mintAddress} on ${cluster}`);
  if (token.data.mintAuthority.__option === "None") fail("the token has no mint authority, so no metadata can be created for it");
  const onChainAuthority = token.data.mintAuthority.value;

  let signer: Signer;
  if (authorityFile) {
    signer = loadSigner("authority", authorityFile);
    if (signer.publicKey !== publicKey(onChainAuthority)) {
      fail(`the authority key (${signer.publicKey}) is not the token's mint authority (${onChainAuthority})`);
    }
  } else {
    signer = createNoopSigner(publicKey(onChainAuthority));
    console.log("No authority keypair: the instruction is built with a placeholder signer.");
  }
  umi.use(signerIdentity(signer));

  const updateAuthority = updateAuthorityArg ? publicKey(asAddress("--update-authority", updateAuthorityArg)) : demoAdmin() ?? signer.publicKey;
  const mint = publicKey(mintAddress);
  const [metadataPda] = findMetadataPda(umi, { mint });

  console.log(`Token:            ${mint}`);
  console.log(`Mint authority:   ${signer.publicKey} (signs and pays)`);
  console.log(`Update authority: ${updateAuthority}`);
  console.log(`Metadata account: ${metadataPda}`);
  console.log(`Name / symbol:    ${name} / ${symbol}`);
  console.log(`URI:              ${uri}\n`);

  const existing = await safeFetchMetadata(umi, metadataPda, { commitment: "confirmed" });
  if (existing) {
    fail(
      `the metadata account already exists (name ${JSON.stringify(existing.name)}, update authority ${existing.updateAuthority}). ` +
        "Change it with updateMetadataAccountV2 signed by its update authority instead.",
    );
  }

  if (skipUriCheck) console.log("URI check skipped (--skip-uri-check).");
  else await checkUri();

  const builder = createMetadataAccountV3(umi, {
    mint,
    mintAuthority: signer,
    updateAuthority,
    data: { name, symbol, uri, sellerFeeBasisPoints: 0, creators: none(), collection: none(), uses: none() },
    isMutable: true,
    collectionDetails: none(),
  });

  if (dryRun) {
    for (const ix of builder.getInstructions()) {
      console.log(`\nInstruction to program ${ix.programId}, ${ix.data.length} bytes of data`);
      for (const key of ix.keys) {
        console.log(`  ${key.pubkey}${key.isSigner ? " signer" : ""}${key.isWritable ? " writable" : ""}`);
      }
    }
    console.log("\nDry run: nothing was sent.");
    return;
  }

  const { value: lamports } = await rpc.getBalance(address(signer.publicKey), { commitment: "confirmed" }).send();
  if (lamports < 10_000_000n) fail("the mint authority needs at least 0.01 SOL for the metadata account's rent and the fee");

  const result = await builder.sendAndConfirm(umi, { confirm: { commitment: "confirmed" } });
  const signature = base58.deserialize(result.signature)[0];
  if (result.result.value.err) fail(`transaction ${signature} failed: ${JSON.stringify(result.result.value.err)}`);

  const created = await safeFetchMetadata(umi, metadataPda, { commitment: "confirmed" });
  if (!created) fail(`transaction ${signature} confirmed but the metadata account was not found`);
  console.log(`Signature:        ${signature}`);
  console.log(`Explorer:         ${explorer("tx", signature)}`);
  console.log(`Metadata:         ${explorer("address", metadataPda)}`);
  const clean = (s: string) => s.replace(/\0+$/, "");
  console.log(`Written:          ${clean(created.name)} / ${clean(created.symbol)} / ${clean(created.uri)}`);
}

main().catch((error: unknown) => {
  fail((error as Error).message ?? String(error));
});
