// SNS SDK v4 (read-only, mainnet): resolve a name and a wallet's primary domain.
// SDK v4 refuses `.sol` once the RPC's finalized slot passes SOL_TLD_CUTOFF_SLOT (452_825_395,
// js/src/config.ts:18 in SolanaNameService/sns-sdk); `.sns` keeps resolving. This script shows both.
import { getPrimaryDomain } from '@bonfida/spl-name-service';
import { resolve, safeResolve } from '@bonfida/spl-name-service/domain';
import { Connection, PublicKey } from '@solana/web3.js';

const RPC_URL = process.env.SOLANA_MAINNET_RPC || 'https://api.mainnet-beta.solana.com';
const connection = new Connection(RPC_URL, 'confirmed');
const names = process.argv.slice(2).length ? process.argv.slice(2) : ['bonfida.sol', 'bonfida.sns'];

async function attempt<T>(label: string, fn: () => Promise<T>): Promise<T | undefined> {
    const t = performance.now();
    try {
        const out = await fn();
        console.log(`${label}: ${String(out)} (${Math.round(performance.now() - t)} ms)`);
        return out;
    } catch (e) {
        console.log(`${label}: ${(e as Error).name}: ${(e as Error).message} (${Math.round(performance.now() - t)} ms)`);
        return undefined;
    }
}

console.log('finalized slot', await connection.getSlot('finalized'));
let owner: PublicKey | undefined;
for (const name of names) {
    await attempt(`resolve(${name})`, () => resolve(connection, name));
    owner = (await attempt(`safeResolve(${name})`, () => safeResolve(connection, name))) ?? owner;
}

if (owner) {
    // A primary domain is only a display hint: `stale` is true when the wallet no longer owns the
    // domain it once set as primary (js/src/primary-domain.ts:146-150). Never show a stale one.
    await attempt(`getPrimaryDomain(${owner.toBase58()})`, async () => {
        const p = await getPrimaryDomain(connection, owner!);
        return `${p.reverse} stale=${p.stale}`;
    });
}
