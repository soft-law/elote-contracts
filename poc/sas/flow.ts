// SAS on devnet: credential + PII-free schema, issue, read, verify, let one expire, revoke.
// The schema matches Moonsong's token-acl-sas-gate (jurisdiction String, verified_at I64), so the
// same attestations could later drive a Token ACL thaw of a series' share mint.
import { readFileSync } from 'node:fs';

import {
    deserializeAttestationData,
    fetchMaybeSchema,
    findAttestationPda,
    findCredentialPda,
    findSchemaPda,
    getCloseAttestationInstruction,
    getCreateAttestationInstruction,
    getCreateCredentialInstruction,
    getCreateSchemaInstruction,
    decodeAttestation,
    SchemaDataType,
    serializeAttestationData,
    SOLANA_ATTESTATION_SERVICE_PROGRAM_ADDRESS,
} from '@solana/attestation';
import {
    type Address,
    type Instruction,
    type KeyPairSigner,
    airdropFactory,
    appendTransactionMessageInstruction,
    createKeyPairSignerFromBytes,
    createSolanaRpc,
    createSolanaRpcSubscriptions,
    createTransactionMessage,
    fetchEncodedAccount,
    generateKeyPairSigner,
    getSignatureFromTransaction,
    lamports,
    pipe,
    sendAndConfirmTransactionFactory,
    setTransactionMessageFeePayerSigner,
    setTransactionMessageLifetimeUsingBlockhash,
    signTransactionMessageWithSigners,
} from '@solana/kit';
import { fetchSysvarClock } from '@solana/sysvars';

const RPC_URL = process.env.SOLANA_DEVNET_RPC || 'https://api.devnet.solana.com';
if (!RPC_URL.includes('devnet')) throw new Error('Devnet only: SOLANA_DEVNET_RPC must point at devnet');
const rpc = createSolanaRpc(RPC_URL);
const rpcSubscriptions = createSolanaRpcSubscriptions(RPC_URL.replace(/^http/, 'ws'));
const sendAndConfirm = sendAndConfirmTransactionFactory({ rpc, rpcSubscriptions });

type Claim = { jurisdiction: string; verified_at: bigint };
const SCHEMA = {
    name: 'ELOTE-INVESTOR-KYC',
    version: 1,
    description: 'Investor passed KYC with the off-chain provider. No personal data: ISO country and review time only.',
    layout: [SchemaDataType.String, SchemaDataType.I64],
    fieldNames: ['jurisdiction', 'verified_at'],
};

const log: { step: string; ms: number; sig?: string }[] = [];
async function timed<T>(step: string, fn: () => Promise<T>): Promise<T> {
    const t = performance.now();
    const out = await fn();
    log.push({ step, ms: Math.round(performance.now() - t), sig: typeof out === 'string' ? out : undefined });
    console.log(`  ${step} (${log.at(-1)!.ms} ms)${typeof out === 'string' ? ` ${out}` : ''}`);
    return out;
}

async function send(payer: KeyPairSigner, ix: Instruction): Promise<string> {
    const { value: blockhash } = await rpc.getLatestBlockhash().send();
    const msg = pipe(
        createTransactionMessage({ version: 0 }),
        m => setTransactionMessageFeePayerSigner(payer, m),
        m => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
        m => appendTransactionMessageInstruction(ix, m),
    );
    const tx = await signTransactionMessageWithSigners(msg);
    await sendAndConfirm(tx as Parameters<typeof sendAndConfirm>[0], { commitment: 'confirmed' });
    return getSignatureFromTransaction(tx);
}

async function loadPayer(): Promise<KeyPairSigner> {
    const path = process.env.SAS_PAYER_KEYPAIR;
    if (path) return createKeyPairSignerFromBytes(new Uint8Array(JSON.parse(readFileSync(path, 'utf8'))));
    const payer = await generateKeyPairSigner();
    await airdropFactory({ rpc, rpcSubscriptions })({
        recipientAddress: payer.address,
        lamports: lamports(500_000_000n),
        commitment: 'confirmed',
    });
    return payer;
}

// What a verifier (backend, or an on-chain gate) must check. Reading the account is not enough:
// SAS stores `expiry` but only enforces it at creation, and revocation means the account is gone.
async function verify(
    credential: Address,
    schema: Address,
    wallet: Address,
    maxAgeSecs: bigint,
): Promise<{ ok: boolean; reason: string; claim?: Claim }> {
    const schemaAcc = await fetchMaybeSchema(rpc, schema);
    if (!schemaAcc.exists) return { ok: false, reason: 'schema missing' };
    if (schemaAcc.programAddress !== SOLANA_ATTESTATION_SERVICE_PROGRAM_ADDRESS) return { ok: false, reason: 'schema not owned by SAS' };
    if (schemaAcc.data.credential !== credential) return { ok: false, reason: 'schema belongs to another credential' };
    if (schemaAcc.data.isPaused) return { ok: false, reason: 'schema paused' };
    // The wallet is bound through the nonce: derive the PDA, never accept an address from the caller.
    const [pda] = await findAttestationPda({ credential, schema, nonce: wallet });
    const encoded = await fetchEncodedAccount(rpc, pda);
    if (!encoded.exists) return { ok: false, reason: 'no attestation (never issued or revoked)' };
    if (encoded.programAddress !== SOLANA_ATTESTATION_SERVICE_PROGRAM_ADDRESS) return { ok: false, reason: 'not owned by SAS' };
    const att = decodeAttestation(encoded);
    if (att.data.credential !== credential || att.data.schema !== schema) return { ok: false, reason: 'credential/schema mismatch' };
    if (att.data.nonce !== wallet) return { ok: false, reason: 'subject mismatch' };
    const { unixTimestamp: now } = await fetchSysvarClock(rpc);
    if (att.data.expiry !== 0n && now >= att.data.expiry) return { ok: false, reason: 'expired (account still exists)' };
    const claim = deserializeAttestationData<Claim>(schemaAcc.data, att.data.data);
    const age = BigInt(now) - BigInt(claim.verified_at);
    if (age < 0n || (maxAgeSecs > 0n && age >= maxAgeSecs)) return { ok: false, reason: 'KYC too old', claim };
    return { ok: true, reason: 'valid', claim };
}

async function main() {
    const started = performance.now();
    console.log(`SAS program ${SOLANA_ATTESTATION_SERVICE_PROGRAM_ADDRESS} on ${RPC_URL}`);
    const payer = await timed('payer ready', loadPayer);
    const authority = payer; // in production: the Squads vault that owns the credential
    const issuerKey = await generateKeyPairSigner(); // in production: the KYC backend's hot key, rotatable
    const investor = await generateKeyPairSigner();
    const shortLived = await generateKeyPairSigner();
    const stranger = await generateKeyPairSigner();

    const credentialName = `ELOTE-POC-${Date.now()}`;
    const [credential] = await findCredentialPda({ authority: authority.address, name: credentialName });
    await timed('create credential', () =>
        send(payer, getCreateCredentialInstruction({ payer, credential, authority, name: credentialName, signers: [issuerKey.address] })),
    );
    const [schema] = await findSchemaPda({ credential, name: SCHEMA.name, version: SCHEMA.version });
    await timed('create schema', () =>
        send(payer, getCreateSchemaInstruction({ payer, authority, credential, schema, name: SCHEMA.name, description: SCHEMA.description, layout: SCHEMA.layout, fieldNames: SCHEMA.fieldNames })),
    );
    const schemaData = (await fetchMaybeSchema(rpc, schema));
    if (!schemaData.exists) throw new Error('schema not found after creation');

    const { unixTimestamp: now } = await fetchSysvarClock(rpc);
    const issue = async (wallet: Address, expiry: bigint) => {
        const [attestation] = await findAttestationPda({ credential, schema, nonce: wallet });
        const data = serializeAttestationData(schemaData.data, { jurisdiction: 'MX', verified_at: BigInt(now) });
        return send(payer, getCreateAttestationInstruction({ payer, authority: issuerKey, credential, schema, attestation, nonce: wallet, expiry, data }));
    };
    await timed('issue attestation (1 year)', () => issue(investor.address, BigInt(now) + 365n * 86_400n));
    await timed('issue attestation (expires in 20 s)', () => issue(shortLived.address, BigInt(now) + 20n));

    const ONE_YEAR = 365n * 86_400n;
    console.log('verify investor   ', await verify(credential, schema, investor.address, ONE_YEAR));
    console.log('verify stranger   ', await verify(credential, schema, stranger.address, ONE_YEAR));
    console.log('verify, max age 1s', await verify(credential, schema, investor.address, 1n));

    console.log('  waiting for the short attestation to expire...');
    await new Promise(r => setTimeout(r, 30_000));
    const expired = await verify(credential, schema, shortLived.address, ONE_YEAR);
    console.log('verify short-lived', expired);
    const [shortPda] = await findAttestationPda({ credential, schema, nonce: shortLived.address });
    console.log('  short-lived account still on chain:', (await fetchEncodedAccount(rpc, shortPda)).exists);

    const [investorPda] = await findAttestationPda({ credential, schema, nonce: investor.address });
    await timed('revoke (close) investor attestation', () =>
        send(payer, getCloseAttestationInstruction({ payer, authority: issuerKey, credential, attestation: investorPda })),
    );
    console.log('verify after revoke', await verify(credential, schema, investor.address, ONE_YEAR));

    console.log(`\ncredential ${credential}\nschema     ${schema}`);
    console.log(`total ${Math.round((performance.now() - started) / 1000)} s`);
}

main().catch(e => {
    console.error(e);
    process.exit(1);
});
