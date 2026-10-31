// Offline checks with a synthetic secret: no network, no account needed.
import assert from 'node:assert/strict';
import { createHmac } from 'node:crypto';
import { test } from 'node:test';

import { signRequest, signedHeaders, verifyWebhook } from './sumsub.ts';

const SECRET = 'poc-synthetic-secret';

test('request signature is HMAC-SHA256 hex over ts + METHOD + path + body', () => {
    const body = JSON.stringify({ externalUserId: 'elote-poc-1' });
    const path = '/resources/applicants?levelName=basic-kyc-level';
    const expected = createHmac('sha256', SECRET).update(`1700000000POST${path}${body}`).digest('hex');
    assert.equal(signRequest(SECRET, 1700000000, 'post', path, body), expected);
});

test('headers carry token, seconds timestamp and signature', () => {
    const h = signedHeaders({ appToken: 'sbx:token', secretKey: SECRET }, 'GET', '/resources/applicants/x/one', '', 1700000000123);
    assert.equal(h['X-App-Token'], 'sbx:token');
    assert.equal(h['X-App-Access-Ts'], '1700000000');
    assert.equal(h['X-App-Access-Sig'], signRequest(SECRET, 1700000000, 'GET', '/resources/applicants/x/one'));
});

test('webhook digest verifies for each supported algorithm and rejects tampering', () => {
    const raw = Buffer.from('{"applicantId":"abc","type":"applicantReviewed","reviewResult":{"reviewAnswer":"GREEN"}}');
    for (const [header, alg] of [
        ['HMAC_SHA1_HEX', 'sha1'],
        ['HMAC_SHA256_HEX', 'sha256'],
        ['HMAC_SHA512_HEX', 'sha512'],
    ] as const) {
        const digest = createHmac(alg, SECRET).update(raw).digest('hex');
        assert.equal(verifyWebhook(SECRET, raw, digest, header), true, header);
        assert.equal(verifyWebhook(SECRET, Buffer.from(raw.toString().replace('GREEN', 'RED')), digest, header), false);
        assert.equal(verifyWebhook('wrong-secret', raw, digest, header), false);
    }
    assert.equal(verifyWebhook(SECRET, raw, undefined, 'HMAC_SHA256_HEX'), false);
    assert.equal(verifyWebhook(SECRET, raw, 'abcd', 'MD5'), false);
    assert.equal(verifyWebhook(SECRET, raw, 'zz', 'HMAC_SHA256_HEX'), false);
});
