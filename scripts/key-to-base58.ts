// Prints the base58 form of a JSON keypair file's 64 secret bytes, the format a wallet's
// "import private key" accepts. For devnet demo keys only: the output is a secret.
//
//   npx tsx scripts/key-to-base58.ts <keypair.json>
import fs from "node:fs";
import { getBase58Decoder } from "@solana/kit";

const file = process.argv[2];
if (!file) {
  console.error("usage: npx tsx scripts/key-to-base58.ts <keypair.json>");
  process.exit(1);
}
let bytes: Uint8Array;
try {
  bytes = Uint8Array.from(JSON.parse(fs.readFileSync(file, "utf8")));
} catch {
  console.error("key-to-base58: the file is not a readable JSON array of bytes");
  process.exit(1);
}
if (bytes.length !== 64) {
  console.error("key-to-base58: a keypair file holds 64 bytes");
  process.exit(1);
}
console.error("Warning: the line below is a secret key. Use this for devnet demo keys only.");
process.stdout.write(getBase58Decoder().decode(bytes) + "\n");
