// The one Metaplex Core instruction a client sends itself: creating the series' collection, whose
// update authority must be the pool PDA before `init_pool` (the program then mints every pagaré
// into it). Encoded by hand to stay on @solana/kit; `test/core.test.ts` pins the bytes against the
// mpl-core crate's builder.
import {
  AccountRole,
  type AccountMeta,
  type AccountSignerMeta,
  type Address,
  type Instruction,
  type TransactionSigner,
  getStructEncoder,
  getU8Encoder,
  getUtf8Encoder,
  addEncoderSizePrefix,
  getU32Encoder,
} from "@solana/kit";
import { MPL_CORE_PROGRAM_ADDRESS } from "./pagare";

export { MPL_CORE_PROGRAM_ADDRESS };

const SYSTEM_PROGRAM_ADDRESS = "11111111111111111111111111111111" as Address;
/** `CreateCollectionV2` in mpl-core's instruction enum. */
const CREATE_COLLECTION_V2 = 21;

const dataEncoder = getStructEncoder([
  ["discriminator", getU8Encoder()],
  ["name", addEncoderSizePrefix(getUtf8Encoder(), getU32Encoder())],
  ["uri", addEncoderSizePrefix(getUtf8Encoder(), getU32Encoder())],
]);

/**
 * `CreateCollectionV2` with no plugins: a bare collection, the only kind `init_pool` accepts. The
 * collection and the payer sign.
 */
export function getCreateCollectionV2Instruction(input: {
  collection: TransactionSigner;
  updateAuthority: Address;
  payer: TransactionSigner;
  name: string;
  uri: string;
}): Instruction<string, readonly (AccountMeta | AccountSignerMeta)[]> {
  const head = dataEncoder.encode({ discriminator: CREATE_COLLECTION_V2, name: input.name, uri: input.uri });
  // plugins: Some(empty vec); external plugin adapters: None.
  const tail = Uint8Array.from([1, 0, 0, 0, 0, 0]);
  const data = new Uint8Array(head.length + tail.length);
  data.set(head);
  data.set(tail, head.length);
  return {
    programAddress: MPL_CORE_PROGRAM_ADDRESS,
    accounts: [
      { address: input.collection.address, role: AccountRole.WRITABLE_SIGNER, signer: input.collection },
      { address: input.updateAuthority, role: AccountRole.READONLY },
      { address: input.payer.address, role: AccountRole.WRITABLE_SIGNER, signer: input.payer },
      { address: SYSTEM_PROGRAM_ADDRESS, role: AccountRole.READONLY },
    ],
    data,
  };
}
