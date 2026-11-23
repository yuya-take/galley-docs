'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');

const { ServiceUnavailableError } = require('../src/errors');
const { createVerifier } = require('../src/jwt');
const { config, issuer, jwksUrl, clock, signingKey, sign, idToken, jsonResponse, fakeFetch } = require('./helpers');

function setup({ keys, jwks } = {}) {
  const time = clock();
  const current = keys || [signingKey('key-1')];
  const served = { keys: current.map((k) => k.jwk) };
  const { fetch, calls } = fakeFetch({ [jwksUrl]: jwks || (() => jsonResponse(200, served)) });
  const verifier = createVerifier({ issuer, clientId: config.clientId, fetch, now: time.now });
  return { time, keys: current, served, calls, verifier };
}

test('有効な ID トークンなら中身を返す', async () => {
  const { time, keys, verifier } = setup();
  const claims = await verifier.verifyIdToken(idToken(keys[0], time.seconds()));
  assert.equal(claims.sub, 'user-1');
});

test('発行元・aud・種類・期限・署名・アルゴリズムが合わないトークンは通さない', async () => {
  const { time, keys, verifier } = setup();
  const now = time.seconds();
  const other = signingKey('key-1');
  const cases = {
    別の発行元: idToken(keys[0], now, { iss: 'https://cognito-idp.ap-northeast-1.amazonaws.com/other' }),
    別のクライアント: idToken(keys[0], now, { aud: 'other-client' }),
    アクセストークン: idToken(keys[0], now, { token_use: 'access' }),
    期限切れ: idToken(keys[0], now - 7200, { exp: now }),
    未来の発行: idToken(keys[0], now + 3600),
    別の鍵の署名: idToken(other, now),
    HS256: sign(keys[0], { iss: issuer, aud: config.clientId, token_use: 'id', exp: now + 60 }, { alg: 'HS256' }),
    'alg が none': sign(keys[0], { iss: issuer, aud: config.clientId, token_use: 'id', exp: now + 60 }, { alg: 'none' }),
    'kid なし': sign(keys[0], { iss: issuer, aud: config.clientId, token_use: 'id', exp: now + 60 }, { kid: undefined }),
    壊れた形: 'not-a-jwt',
    空: '',
  };
  for (const [name, token] of Object.entries(cases)) {
    assert.equal(await verifier.verifyIdToken(token), null, name);
  }
  assert.equal(await verifier.verifyIdToken(undefined), null);
});

test('JWKS は使い回し、リクエストのたびには取りに行かない', async () => {
  const { time, keys, calls, verifier } = setup();
  for (let i = 0; i < 3; i += 1) {
    assert.ok(await verifier.verifyIdToken(idToken(keys[0], time.seconds())));
    time.advance(600);
  }
  assert.equal(calls.length, 1);
});

test('鍵が入れ替わったら、知らない kid のときだけ取り直す', async () => {
  const { time, keys, served, calls, verifier } = setup();
  assert.ok(await verifier.verifyIdToken(idToken(keys[0], time.seconds())));
  const rotated = signingKey('key-2');
  served.keys.push(rotated.jwk);
  time.advance(120);
  assert.ok(await verifier.verifyIdToken(idToken(rotated, time.seconds())));
  assert.equal(calls.length, 2);
});

test('知らない kid が続いても、取り直すのは 1 分に 1 回まで', async () => {
  const { time, keys, calls, verifier } = setup();
  assert.ok(await verifier.verifyIdToken(idToken(keys[0], time.seconds())));
  time.advance(120);
  const unknown = signingKey('unknown');
  for (let i = 0; i < 5; i += 1) {
    assert.equal(await verifier.verifyIdToken(idToken(unknown, time.seconds())), null);
  }
  assert.equal(calls.length, 2);
  time.advance(61);
  assert.equal(await verifier.verifyIdToken(idToken(unknown, time.seconds())), null);
  assert.equal(calls.length, 3);
});

test('期限切れのトークンでは JWKS を取りに行かない', async () => {
  const { time, keys, calls, verifier } = setup();
  const now = time.seconds();
  assert.equal(await verifier.verifyIdToken(idToken(keys[0], now - 7200, { exp: now - 1 })), null);
  assert.equal(calls.length, 0);
});

test('JWKS を取れなかった直後は、取り直さずに ServiceUnavailableError にし、1 分たてば取り直す', async () => {
  let fail = true;
  const keys = [signingKey('key-1')];
  const { time, calls, verifier } = setup({
    keys,
    jwks: () => (fail ? jsonResponse(503, {}) : jsonResponse(200, { keys: keys.map((k) => k.jwk) })),
  });
  await assert.rejects(verifier.verifyIdToken(idToken(keys[0], time.seconds())), ServiceUnavailableError);
  await assert.rejects(verifier.verifyIdToken(idToken(keys[0], time.seconds())), ServiceUnavailableError);
  assert.equal(calls.length, 1);
  fail = false;
  time.advance(61);
  assert.ok(await verifier.verifyIdToken(idToken(keys[0], time.seconds())));
  assert.equal(calls.length, 2);
});

test('JWKS を取れないときは ServiceUnavailableError にする', async () => {
  for (const jwks of [
    () => {
      throw new TypeError('fetch failed');
    },
    () => jsonResponse(500, {}),
    () => new Response('not json', { status: 200 }),
  ]) {
    const { time, keys, verifier } = setup({ jwks });
    await assert.rejects(verifier.verifyIdToken(idToken(keys[0], time.seconds())), ServiceUnavailableError);
  }
});

test('allowExpired では期限だけを見ず、署名・発行元・aud・種類は確かめる', async () => {
  const { time, keys, verifier } = setup();
  const now = time.seconds();
  const expired = idToken(keys[0], now - 7200, { exp: now - 3600 });
  assert.equal(await verifier.verifyIdToken(expired), null);
  assert.equal((await verifier.verifyIdToken(expired, { allowExpired: true })).sub, 'user-1');
  for (const token of [
    idToken(signingKey('key-1'), now - 7200, { exp: now - 3600 }),
    idToken(keys[0], now - 7200, { exp: now - 3600, aud: 'other-client' }),
    idToken(keys[0], now - 7200, { exp: now - 3600, token_use: 'access' }),
  ]) {
    assert.equal(await verifier.verifyIdToken(token, { allowExpired: true }), null);
  }
});
