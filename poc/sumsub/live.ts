// Live sandbox run: create an applicant, then a WebSDK access token for it. Needs SUMSUB_* in poc/.env.
import { randomUUID } from 'node:crypto';

import { createApplicant, createWebSdkAccessToken } from './sumsub.ts';

const { SUMSUB_APP_TOKEN, SUMSUB_SECRET_KEY, SUMSUB_LEVEL_NAME = 'basic-kyc-level' } = process.env;
if (!SUMSUB_APP_TOKEN || !SUMSUB_SECRET_KEY) {
    console.error('Set SUMSUB_APP_TOKEN and SUMSUB_SECRET_KEY (sandbox) in poc/.env');
    process.exit(1);
}
if (!SUMSUB_APP_TOKEN.startsWith('sbx:')) {
    console.error('Refusing to run: the app token is not a sandbox token (sbx:...)');
    process.exit(1);
}

const creds = { appToken: SUMSUB_APP_TOKEN, secretKey: SUMSUB_SECRET_KEY };
const started = performance.now();
const externalUserId = `elote-poc-${randomUUID()}`;
const applicant = await createApplicant(creds, externalUserId, SUMSUB_LEVEL_NAME);
console.log('applicant', applicant.id, applicant.review?.reviewStatus);
const token = await createWebSdkAccessToken(creds, externalUserId, SUMSUB_LEVEL_NAME);
console.log('websdk token issued for', token.userId, `(${token.token.length} chars, not printed)`);
console.log(`done in ${Math.round(performance.now() - started)} ms`);
