'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');

const { createHandler, valuesFromEnv } = require('../src/index');

function event(value) {
  return { version: '2.0', type: 'REQUEST', headers: value === undefined ? {} : { 'x-origin-verify': value } };
}

test('CloudFront が付けた値なら通す', async () => {
  const handler = createHandler(['current-secret']);
  assert.deepEqual(await handler(event('current-secret')), { isAuthorized: true });
});

test('値が無い・違うときは断る', async () => {
  const handler = createHandler(['current-secret']);
  for (const value of [undefined, '', 'current', 'current-secret ', 'CURRENT-SECRET', 'current-secret-x']) {
    assert.deepEqual(await handler(event(value)), { isAuthorized: false }, String(value));
  }
  assert.deepEqual(await handler({}), { isAuthorized: false });
});

test('入れ替えの間は、古い値と新しい値の両方を通す', async () => {
  const handler = createHandler(valuesFromEnv({ ORIGIN_VERIFY_VALUES: 'new-secret, old-secret' }));
  assert.equal((await handler(event('new-secret'))).isAuthorized, true);
  assert.equal((await handler(event('old-secret'))).isAuthorized, true);
  assert.equal((await handler(event('other'))).isAuthorized, false);
});

test('値が設定されていなければ、すべて断る', async () => {
  for (const env of [{}, { ORIGIN_VERIFY_VALUES: '' }, { ORIGIN_VERIFY_VALUES: ' , ' }]) {
    const handler = createHandler(valuesFromEnv(env));
    assert.equal((await handler(event(''))).isAuthorized, false);
    assert.equal((await handler(event('anything'))).isAuthorized, false);
  }
});
