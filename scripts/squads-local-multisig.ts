// Writes an autonomous Squads v4 multisig as an account file for a local validator's
// `--account <address> <file>` preload, so the local e2e has a council without Squads'
// ProgramConfig (which multisig_create_v2 needs and a fresh validator lacks). It sends nothing and
// prints the multisig's address on the first line, its vault (the council) on the second.
//
//   npx tsx scripts/squads-local-multisig.ts --members <a.json,b.json,c.json> --threshold <n> --out <file>
//
// Every member gets every permission. Local tooling only: a multisig on a real cluster is created
// through Squads.
import fs from "node:fs";
import { generateKeyPairSigner } from "@solana/kit";
import { checkFlags, fail, integer, loadSigner, required } from "./lib/cli.ts";
import { PERM_EXECUTE, PERM_INITIATE, PERM_VOTE, SQUADS_PROGRAM_ADDRESS, encodeMultisig, multisigPda, vaultPda } from "./lib/squads.ts";

checkFlags(["members", "threshold", "out"]);
const files = required("members").split(",");
const threshold = integer("threshold", undefined, 1, files.length) ?? fail("--threshold is required");
const members = await Promise.all(files.map((f, i) => loadSigner(`member ${i + 1}`, f)));
const createKey = (await generateKeyPairSigner()).address;
const [multisig, bump] = await multisigPda(createKey);
const data = encodeMultisig(
  createKey,
  bump,
  threshold,
  members.map((m) => ({ key: m.address, permissions: PERM_INITIATE | PERM_VOTE | PERM_EXECUTE }))
);
const account = {
  pubkey: multisig,
  account: {
    lamports: 1_000_000_000,
    data: [Buffer.from(data).toString("base64"), "base64"],
    owner: SQUADS_PROGRAM_ADDRESS,
    executable: false,
    rentEpoch: 0,
    space: data.length,
  },
};
fs.writeFileSync(required("out"), JSON.stringify(account));
console.log(multisig);
console.log((await vaultPda(multisig))[0]);
