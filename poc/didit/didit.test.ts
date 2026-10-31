// Offline checks with a synthetic secret. The canonical form is checked against output produced by
// Didit's own Python reference (json.dumps with sort_keys, compact separators, ensure_ascii=False).
import assert from 'node:assert/strict';
import { createHmac } from 'node:crypto';
import { test } from 'node:test';

import { canonicalJson, verifyWebhook } from './didit.ts';

const SECRET = 'poc-synthetic-secret';
const NOW = 1_800_000_000;
const RAW = Buffer.from(
    '{"webhook_type": "status.updated", "session_id": "s-1", "status": "Approved", "timestamp": 1800000000,\n "vendor_data": "elote-poc-1", "decision": {"aml": {"score": 87.0, "hits": []}, "name": "José"}}',
);
// python3 -c 'import json;print(json.dumps(shorten_floats(json.loads(RAW)),sort_keys=True,separators=(",",":"),ensure_ascii=False))'
const PY_CANONICAL =
    '{"decision":{"aml":{"hits":[],"score":87},"name":"José"},"session_id":"s-1","status":"Approved","timestamp":1800000000,"vendor_data":"elote-poc-1","webhook_type":"status.updated"}';

test('canonical JSON matches the Python reference', () => {
    assert.equal(canonicalJson(JSON.parse(RAW.toString())), PY_CANONICAL);
});

test('X-Signature-V2 over canonical JSON verifies; tampering and stale timestamps fail', () => {
    const sig = createHmac('sha256', SECRET).update(PY_CANONICAL).digest('hex');
    const headers = { 'X-Timestamp': String(NOW), 'X-Signature-V2': sig };
    assert.equal(verifyWebhook(SECRET, RAW, headers, NOW), true);
    assert.equal(verifyWebhook(SECRET, Buffer.from(RAW.toString().replace('Approved', 'Declined')), headers, NOW), false);
    assert.equal(verifyWebhook(SECRET, RAW, headers, NOW + 301), false);
    assert.equal(verifyWebhook('wrong', RAW, headers, NOW), false);
});

test('X-Signature over raw bytes verifies as a fallback', () => {
    const sig = createHmac('sha256', SECRET).update(RAW).digest('hex');
    assert.equal(verifyWebhook(SECRET, RAW, { 'x-timestamp': String(NOW), 'x-signature': sig }, NOW), true);
    assert.equal(verifyWebhook(SECRET, RAW, { 'x-timestamp': String(NOW) }, NOW), false);
});
