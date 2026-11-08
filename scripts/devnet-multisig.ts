// Creates a real autonomous Squads v4 multisig on a live cluster (devnet) through
// multisig_create_v2: the program's own creation path, with its ProgramConfig and treasury fee.
// The vault 0 printed on the second line is the council init_pool takes. For a local validator use
// squads-local-multisig.ts, which preloads the account instead (a fresh validator has no
// ProgramConfig).
//
//   npx tsx scripts/devnet-multisig.ts --cluster devnet --members <a.json,b.json,c.json>
//        --threshold <n> [--payer-keypair <path>] [--yes]
//
// Every member gets every permission (initiate, vote, execute). The payer (default
// ELOTE_ADMIN_KEYPAIR or .demo-keys/admin.json) funds the rent and the creation fee.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { AccountRole, type Address, generateKeyPairSigner, getAddressDecoder, getAddressEncoder } from "@solana/kit";
import { createHash } from "node:crypto";
import { arg, checkFlags, cluster, confirmCluster, fail, integer, loadSigner, required, sendOrFail } from "./lib/cli.ts";
import {
  PERM_EXECUTE,
  PERM_INITIATE,
  PERM_VOTE,
  SQUADS_PROGRAM_ADDRESS,
  multisigPda,
  programConfigPda,
  vaultPda,
} from "./lib/squads.ts";

checkFlags(["cluster", "members", "threshold", "payer-keypair", "yes"]);
const c = cluster();
const files = required("members").split(",");
const threshold = integer("threshold", undefined, 1, files.length) ?? fail("--threshold is required");
const members = await Promise.all(files.map((f, i) => loadSigner(`member ${i + 1}`, f)));
const payer = await loadSigner("payer", arg("payer-keypair") ?? process.env.ELOTE_ADMIN_KEYPAIR ?? ".demo-keys/admin.json");
await confirmCluster(c, "multisig_create_v2");

const createKey = await generateKeyPairSigner();
const [multisig] = await multisigPda(createKey.address);
const [programConfig] = await programConfigPda();
// ProgramConfig: discriminator(8) || authority(32) || multisig_creation_fee(u64) || treasury(32).
const config = await c.rpc.getAccountInfo(programConfig, { encoding: "base64", commitment: "confirmed" }).send();
if (!config.value) fail(`Squads ProgramConfig ${programConfig} not found on this cluster`);
const raw = Buffer.from(config.value!.data[0], "base64");
const treasury = getAddressDecoder().decode(raw.subarray(48, 80));
const fee = raw.readBigUInt64LE(40);
console.log(`creating ${multisig} (create key ${createKey.address}); creation fee ${fee} lamports to the Squads treasury`);

const u16 = (n: number) => { const b = Buffer.alloc(2); b.writeUInt16LE(n); return b; };
const u32 = (n: number) => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
const enc = getAddressEncoder();
const data = Buffer.concat([
  createHash("sha256").update("global:multisig_create_v2").digest().subarray(0, 8),
  Buffer.of(0), // config_authority: None (autonomous)
  u16(threshold),
  u32(members.length),
  ...members.map((m) => Buffer.concat([Buffer.from(enc.encode(m.address)), Buffer.of(PERM_INITIATE | PERM_VOTE | PERM_EXECUTE)])),
  u32(0), // time_lock
  Buffer.of(0), // rent_collector: None
  Buffer.of(0), // memo: None
]);
const ix = {
  programAddress: SQUADS_PROGRAM_ADDRESS,
  accounts: [
    { address: programConfig, role: AccountRole.READONLY },
    { address: treasury, role: AccountRole.WRITABLE },
    { address: multisig, role: AccountRole.WRITABLE },
    { address: createKey.address, role: AccountRole.READONLY_SIGNER, signer: createKey },
    { address: payer.address, role: AccountRole.WRITABLE_SIGNER, signer: payer },
    { address: "11111111111111111111111111111111" as Address, role: AccountRole.READONLY },
  ],
  data: Uint8Array.from(data),
};
await sendOrFail(c, `multisig_create_v2 (${threshold} of ${files.length})`, payer, [ix]);
console.log(multisig);
console.log((await vaultPda(multisig))[0]);
console.log(`\nDemo on Solana devnet with test assets. Not an offer of securities.`);
