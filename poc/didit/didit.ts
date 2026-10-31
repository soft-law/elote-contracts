// Didit: create a hosted verification session and verify webhooks.
// Reference: didit-protocol/skills, skills/didit-verification-management/SKILL.md (session :287-335,
// signature headers :774-781, Python reference verifier :785-845) and https://docs.didit.me/integration/webhooks.
import { createHmac, timingSafeEqual } from 'node:crypto';

export const DIDIT_BASE_URL = 'https://verification.didit.me/v3';

export async function createSession(apiKey: string, workflowId: string, vendorData: string, callback: string) {
    const res = await fetch(`${DIDIT_BASE_URL}/session/`, {
        method: 'POST',
        headers: { 'x-api-key': apiKey, 'Content-Type': 'application/json', Accept: 'application/json' },
        body: JSON.stringify({ workflow_id: workflowId, vendor_data: vendorData, callback }),
    });
    const text = await res.text();
    if (!res.ok) throw new Error(`Didit POST /session/ -> ${res.status}: ${text}`);
    return JSON.parse(text) as { session_id: string; session_token: string; url: string; status: string };
}

// Didit signs Python's json.dumps(shorten_floats(body), sort_keys=True, separators=(",", ":"),
// ensure_ascii=False). JSON.stringify already prints 87.0 as 87 and uses the same separators, so the
// only work is sorting keys. Known divergence, not covered: Python writes 1e-07 where JS writes 1e-7,
// and key order differs for characters outside the BMP. Fall back to X-Signature (raw body) then.
export function canonicalJson(value: unknown): string {
    const sort = (v: unknown): unknown => {
        if (Array.isArray(v)) return v.map(sort);
        if (v && typeof v === 'object') {
            return Object.fromEntries(Object.keys(v).sort().map(k => [k, sort((v as Record<string, unknown>)[k])]));
        }
        return v;
    };
    return JSON.stringify(sort(value));
}

const hmacHex = (secret: string, data: string | Buffer) => createHmac('sha256', secret).update(data).digest('hex');
const sameHex = (a: string | undefined, b: string) =>
    !!a && a.length === b.length && timingSafeEqual(Buffer.from(a), Buffer.from(b));

export function verifyWebhook(
    secret: string,
    rawBody: Buffer,
    headers: Record<string, string | undefined>,
    nowSecs = Math.floor(Date.now() / 1000),
    toleranceSecs = 300,
): boolean {
    const h = Object.fromEntries(Object.entries(headers).map(([k, v]) => [k.toLowerCase(), v]));
    const ts = Number.parseInt(h['x-timestamp'] ?? '', 10);
    if (!Number.isFinite(ts) || Math.abs(nowSecs - ts) > toleranceSecs) return false;
    try {
        if (sameHex(h['x-signature-v2'], hmacHex(secret, canonicalJson(JSON.parse(rawBody.toString('utf8')))))) return true;
    } catch {
        // not JSON: only the raw-body signature can match
    }
    return sameHex(h['x-signature'], hmacHex(secret, rawBody));
}
