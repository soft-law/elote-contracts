// Sumsub request signing, the two calls the onboarding needs, and webhook verification.
// Signing follows https://docs.sumsub.com/reference/authentication: HMAC-SHA256 (hex) over
// ts + METHOD + path?query + body, sent as X-App-Access-Sig with X-App-Token and X-App-Access-Ts.
import { createHmac, timingSafeEqual } from 'node:crypto';

export const SUMSUB_BASE_URL = 'https://api.sumsub.com';

export type Credentials = { appToken: string; secretKey: string };

export function signRequest(secretKey: string, ts: number, method: string, pathWithQuery: string, body = ''): string {
    return createHmac('sha256', secretKey)
        .update(`${ts}${method.toUpperCase()}${pathWithQuery}${body}`)
        .digest('hex');
}

export function signedHeaders(creds: Credentials, method: string, pathWithQuery: string, body = '', now = Date.now()) {
    const ts = Math.floor(now / 1000);
    return {
        'X-App-Token': creds.appToken,
        'X-App-Access-Ts': String(ts),
        'X-App-Access-Sig': signRequest(creds.secretKey, ts, method, pathWithQuery, body),
        Accept: 'application/json',
        'Content-Type': 'application/json',
    };
}

async function call<T>(creds: Credentials, method: string, pathWithQuery: string, payload?: unknown): Promise<T> {
    const body = payload === undefined ? '' : JSON.stringify(payload);
    const res = await fetch(SUMSUB_BASE_URL + pathWithQuery, {
        method,
        headers: signedHeaders(creds, method, pathWithQuery, body),
        body: body || undefined,
    });
    const text = await res.text();
    if (!res.ok) throw new Error(`Sumsub ${method} ${pathWithQuery} -> ${res.status}: ${text}`);
    return JSON.parse(text) as T;
}

// `externalUserId` is our own opaque id; never a wallet address or anything that identifies the person.
export function createApplicant(creds: Credentials, externalUserId: string, levelName: string) {
    const path = `/resources/applicants?levelName=${encodeURIComponent(levelName)}`;
    return call<{ id: string; externalUserId: string; review?: { reviewStatus: string } }>(creds, 'POST', path, {
        externalUserId,
    });
}

export function createWebSdkAccessToken(creds: Credentials, userId: string, levelName: string, ttlInSecs = 600) {
    return call<{ token: string; userId: string }>(creds, 'POST', '/resources/accessTokens/sdk', {
        userId,
        levelName,
        ttlInSecs,
    });
}

// Hands a verified applicant to a partner (Etherfuse imports it at
// POST /organization/{id}/verification/import). Single-use and short-lived; needs a data-sharing
// agreement and Etherfuse added as a sharing partner on both sides.
export function createShareToken(creds: Credentials, applicantId: string, forClientId: string, ttlInSecs = 600) {
    return call<{ token: string; forClientId: string }>(creds, 'POST', '/resources/accessTokens/shareToken', {
        applicantId,
        forClientId,
        ttlInSecs,
    });
}

const DIGEST_ALGS = { HMAC_SHA1_HEX: 'sha1', HMAC_SHA256_HEX: 'sha256', HMAC_SHA512_HEX: 'sha512' } as const;

// Verifies X-Payload-Digest against the raw request body. The body must be the exact bytes received:
// re-serialising parsed JSON changes whitespace and breaks the digest.
export function verifyWebhook(
    secret: string,
    rawBody: Buffer | string,
    digestHeader: string | undefined,
    algHeader: string | undefined,
): boolean {
    if (!digestHeader || !algHeader) return false;
    const alg = DIGEST_ALGS[algHeader as keyof typeof DIGEST_ALGS];
    if (!alg) return false;
    const expected = createHmac(alg, secret).update(rawBody).digest();
    // Buffer.from(x, 'hex') never throws, it truncates at the first bad character, so validate first.
    if (!/^[0-9a-fA-F]+$/.test(digestHeader) || digestHeader.length !== expected.length * 2) return false;
    return timingSafeEqual(Buffer.from(digestHeader, 'hex'), expected);
}
