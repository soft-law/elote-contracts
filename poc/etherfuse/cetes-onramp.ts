// Etherfuse sandbox: can a Solana wallet receive CETES through an MXN onramp, and what does the
// flow cost? Lists the Solana assets, registers a dummy CLABE and a devnet wallet under the
// partner's own (sandbox, self-approved) organization, quotes and orders 100 MXN of CETES,
// simulates the SPEI deposit and waits for `completed`. Not run yet: needs ETHERFUSE_SANDBOX_API_KEY.
// Docs: v2/guides/testing-onramps, v2/overview#the-flow. Sandbox onramps are capped at 500 MXN.
const BASE = 'https://api.sand.etherfuse.com';
const key = process.env.ETHERFUSE_SANDBOX_API_KEY;
const org = process.env.ETHERFUSE_ORG_ID; // the partner's own organization id, from GET /me
const clabe = process.env.ETHERFUSE_CLABE ?? '012180000000000002'; // sandbox dummy
const wallet = process.env.ETHERFUSE_WALLET; // a devnet public key you control
if (!key || !org || !wallet) throw new Error('set ETHERFUSE_SANDBOX_API_KEY, ETHERFUSE_ORG_ID and ETHERFUSE_WALLET in poc/.env');

async function api<T>(method: string, path: string, body?: unknown): Promise<T> {
    const res = await fetch(BASE + path, {
        method,
        headers: { Authorization: key!, 'Content-Type': 'application/json', Accept: 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await res.text();
    if (!res.ok) throw new Error(`${method} ${path} -> ${res.status}: ${text}`);
    return (text ? JSON.parse(text) : {}) as T;
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const t0 = performance.now();
const log = (s: string) => console.log(`[${Math.round(performance.now() - t0)} ms] ${s}`);

// 1. Which tokens a Solana wallet can receive here (answers the open question from 2026-10-19).
const assets = await api<{ tokens?: { asset: string; symbol?: string; currency?: string }[] }>('GET', '/supported_assets?chain=solana');
const tokens = assets.tokens ?? [];
log(`solana tokens: ${tokens.map((t) => `${t.symbol ?? '?'}=${t.asset}`).join(', ') || JSON.stringify(assets).slice(0, 300)}`);
const cetes = tokens.find((t) => (t.symbol ?? '').toUpperCase() === 'CETES');
if (!cetes) throw new Error('no CETES on Solana in this environment');

// 2. Payment accounts: the paying CLABE and the receiving wallet, both under the same organization.
type Account = { id: string; status: string; rails: Record<string, { source?: { status: string } | null; destination?: { status: string } | null }> };
const bank = await api<Account>('POST', `/organization/${org}/payment_account`, {
    kind: 'bank', label: 'poc clabe', country: 'MX', currency: 'MXN',
    accountDetails: { kind: 'clabe', clabe },
    registrationInfo: { taxId: 'XAXX010101000' }, // sandbox dummy RFC per the docs
});
const dest = await api<Account>('POST', `/organization/${org}/payment_account`, {
    kind: 'wallet', label: 'poc devnet wallet', accountDetails: { kind: 'address', address: wallet, chain: 'solana' },
});
const ready = async (id: string, dir: 'source' | 'destination') => {
    for (let i = 0; i < 20; i++) {
        const a = await api<Account>('GET', `/organization/${org}/payment_account/${id}`);
        const rail = Object.values(a.rails).find((r) => r[dir]?.status === 'ready');
        if (rail) return;
        await sleep(1500);
    }
    throw new Error(`payment account ${id} not ready as ${dir}`);
};
await ready(bank.id, 'source');
await ready(dest.id, 'destination');
log(`accounts ready: bank ${bank.id}, wallet ${dest.id}`);

// 3. Quote and order 100 MXN -> CETES. The quote lives 2 minutes; the order id equals the quote id.
const quote = await api<{ quoteId: string; destinationAmount: string; feeBps: number; feeAmount: string; fees: unknown[]; expiresAt: string }>(
    'POST', `/organization/${org}/quote`,
    { source: { type: 'stored', id: bank.id }, destination: { type: 'stored', id: dest.id }, sourceAmount: '100', sourceAsset: 'MXN', destinationAsset: cetes.asset },
);
log(`quote ${quote.quoteId}: 100 MXN -> ${quote.destinationAmount} CETES, fee ${quote.feeBps} bps (${quote.feeAmount} MXN), lines ${JSON.stringify(quote.fees)}`);
const order = await api<{ id: string; status: string }>('POST', `/organization/${org}/order`, { quoteId: quote.quoteId, memo: 'elote poc' });
log(`order ${order.id} ${order.status}`);

// 4. Sandbox only: simulate the SPEI deposit of the exact amount, then wait for delivery.
await api('POST', `/organization/${org}/order/${order.id}/fiat_received`, { amount: '100' });
for (let i = 0; i < 40; i++) {
    const o = await api<{ status: string; destination?: unknown; failure?: unknown; fundingInstructions?: unknown }>('GET', `/organization/${org}/order/${order.id}`);
    log(`order ${o.status}`);
    if (o.status === 'completed') { console.log(JSON.stringify(o.destination ?? o, null, 2)); break; }
    if (o.status === 'failed' || o.status === 'canceled') throw new Error(JSON.stringify(o.failure ?? o));
    await sleep(3000);
}
