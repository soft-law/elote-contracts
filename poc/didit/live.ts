// Live sandbox run: create a hosted verification session. Needs DIDIT_* in poc/.env.
import { randomUUID } from 'node:crypto';

import { createSession } from './didit.ts';

const { DIDIT_API_KEY, DIDIT_WORKFLOW_ID } = process.env;
if (!DIDIT_API_KEY || !DIDIT_WORKFLOW_ID) {
    console.error('Set DIDIT_API_KEY and DIDIT_WORKFLOW_ID (sandbox application) in poc/.env');
    process.exit(1);
}

const started = performance.now();
const session = await createSession(DIDIT_API_KEY, DIDIT_WORKFLOW_ID, `elote-poc-${randomUUID()}`, 'https://example.invalid/kyc/done');
console.log('session', session.session_id, session.status, 'url', session.url);
console.log(`done in ${Math.round(performance.now() - started)} ms`);
