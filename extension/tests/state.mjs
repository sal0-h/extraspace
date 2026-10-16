import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
const source = readFileSync(new URL('../state.js', import.meta.url), 'utf8');
const {describeSession} = await import(`data:text/javascript,${encodeURIComponent(source)}`);

assert.equal(describeSession(false, '{"Streaming":{}}').checked, false);
assert.equal(describeSession(false, null).subtitle, 'Off');
assert.equal(describeSession(true, '"NoTablet"').subtitle, 'Waiting');
assert.equal(describeSession(true, 'invalid JSON').checked, true);
assert.equal(describeSession(false, null, 'on').subtitle, 'Starting…');
assert.equal(describeSession(true, null, 'off').checked, false);
assert.equal(describeSession(true, '{"Connecting":{"step":"Finding tablet"}}').detail, 'Finding tablet');
assert.equal(describeSession(true, '{"Unauthorized":{"device":"Tablet"}}').subtitle, 'Allow USB');
assert.equal(describeSession(true, '{"Failed":{"message":"private diagnostic"}}').subtitle, 'Needs attention');
for (const connection of ['Wi-Fi', 'USB']) {
    const state = {Streaming: {device: 'My tablet', connection, width: 1536, height: 960}};
    const status = describeSession(true, JSON.stringify(state));
    assert.equal(status.subtitle, connection);
    assert.equal(status.detail, `Connected to My tablet over ${connection} — 1536 × 960`);
    assert.equal(status.checked, true);
}
console.log('Quick Settings state checks passed');
