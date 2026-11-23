'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');

const { createHandler, ID_COOKIE, REFRESH_COOKIE, LOGIN_COOKIE_PREFIX } = require('../src/handler');
const {
  config,
  jwksUrl,
  tokenUrl,
  revokeUrl,
  clock,
  signingKey,
  idToken,
  jsonResponse,
  fakeFetch,
  viewerRequest,
  originResponse,
  header,
  setCookies,
} = require('./helpers');

const HASH = 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855';
/** Cognito のリフレッシュトークンの形（暗号化された JWE。5 つの部分）を真似た値。 */
const REFRESH = ['eyJjdHkiOiJKV1QiLCJlbmMiOiJBMjU2R0NNIiwiYWxnIjoiUlNBLU9BRVAifQ', 'a'.repeat(342), 'b'.repeat(16), 'c'.repeat(1200), 'd'.repeat(22)].join('.');
const REFRESH_2 = REFRESH.replace('a', 'e');

/**
 * 関数とまわりを用意する。`token` は Cognito のトークンのエンドポイントの応答を作る関数。
 */
function setup({ token, jwks } = {}) {
  const time = clock();
  const key = signingKey('key-1');
  const { fetch, calls } = fakeFetch({
    [jwksUrl]: jwks || (() => jsonResponse(200, { keys: [key.jwk] })),
    [tokenUrl]: token || (() => jsonResponse(400, { error: 'invalid_grant' })),
    [revokeUrl]: () => new Response('', { status: 200 }),
  });
  const handler = createHandler(config, { fetch, now: time.now });
  return { time, key, calls, handler };
}

function isRequest(result) {
  return result.status === undefined && typeof result.uri === 'string';
}

function cookieNamed(response, name) {
  return setCookies(response).find((c) => c.name === name);
}

/** 期限が 1 時間の ID トークンと、そのまわり。 */
function loggedIn(setupResult, overrides = {}) {
  return idToken(setupResult.key, setupResult.time.seconds(), overrides);
}

test('有効な ID トークンがあれば、画面のリクエストをそのまま通す', async () => {
  const s = setup();
  const event = viewerRequest({ uri: '/demo', cookies: { [ID_COOKIE]: loggedIn(s), galley_author: 'x' } });
  const result = await s.handler(event);
  assert.ok(isRequest(result));
  assert.equal(result.uri, '/demo');
});

test('ログインしていなければ、PKCE を付けて Cognito のログインに回す', async () => {
  const s = setup();
  const response = await s.handler(viewerRequest({ uri: '/demo/keikaku', querystring: 'v=2' }));
  assert.equal(response.status, '302');
  const location = new URL(header(response, 'location')[0]);
  assert.equal(location.origin, `https://${config.cognitoDomain}`);
  assert.equal(location.pathname, '/oauth2/authorize');
  assert.equal(location.searchParams.get('client_id'), config.clientId);
  assert.equal(location.searchParams.get('redirect_uri'), 'https://galley.example.com/_auth/callback');
  assert.equal(location.searchParams.get('code_challenge_method'), 'S256');
  assert.equal(location.searchParams.get('scope'), 'openid email profile');
  const state = location.searchParams.get('state');
  const login = cookieNamed(response, `${LOGIN_COOKIE_PREFIX}${state}`);
  assert.ok(login, 'state ごとの Cookie を出す');
  assert.deepEqual(login.attributes, [
    'Domain=galley.example.com',
    'Path=/_auth/callback',
    'Max-Age=600',
    'Secure',
    'HttpOnly',
    'SameSite=Lax',
  ]);
  const saved = JSON.parse(Buffer.from(login.value, 'base64url').toString('utf8'));
  assert.equal(saved.r, 'https://galley.example.com/demo/keikaku?v=2');
  assert.equal(header(response, 'cache-control')[0], 'no-store');
});

test('ログインの画面に回すのは GET の移動だけ。iframe と fetch には回さない', async () => {
  const s = setup();
  const iframe = await s.handler(
    viewerRequest({ host: config.docsHost, uri: `/b/${HASH}`, headers: { 'sec-fetch-dest': 'iframe', 'sec-fetch-mode': 'navigate' } }),
  );
  assert.equal(iframe.status, '401');
  assert.match(iframe.body, /ログインし直してください/);
  const post = await s.handler(viewerRequest({ uri: '/_topcoat/call', method: 'POST', headers: { 'sec-fetch-mode': 'cors' } }));
  assert.equal(post.status, '401');
  const script = await s.handler(viewerRequest({ uri: '/_topcoat/assets/app.js', headers: { 'sec-fetch-mode': 'no-cors' } }));
  assert.equal(script.status, '401');
  const navigate = await s.handler(viewerRequest({ uri: '/', headers: { 'sec-fetch-mode': 'navigate', 'sec-fetch-dest': 'document' } }));
  assert.equal(navigate.status, '302');
});

test('資料配信で関数が返す応答にも CSP の sandbox を付ける', async () => {
  const s = setup();
  for (const event of [
    viewerRequest({ host: config.docsHost, uri: `/b/${HASH}` }),
    viewerRequest({ host: config.docsHost, uri: `/b/${HASH}`, headers: { 'sec-fetch-dest': 'iframe' } }),
  ]) {
    const response = await s.handler(event);
    assert.equal(header(response, 'content-security-policy')[0], config.docsCsp);
    assert.equal(header(response, 'x-content-type-options')[0], 'nosniff');
    assert.equal(header(response, 'referrer-policy')[0], 'no-referrer');
  }
});

test('戻り先で、コードをトークンに換えて Cookie を出し、元の URL に戻す', async () => {
  let tokenRequest;
  const s = setup({
    token: (body) => {
      tokenRequest = body;
      return jsonResponse(200, {
        id_token: idToken(s.key, s.time.seconds()),
        access_token: 'access',
        refresh_token: REFRESH,
        expires_in: 3600,
      });
    },
  });
  const start = await s.handler(viewerRequest({ host: config.docsHost, uri: `/b/${HASH}` }));
  const state = new URL(header(start, 'location')[0]).searchParams.get('state');
  const login = cookieNamed(start, `${LOGIN_COOKIE_PREFIX}${state}`);

  const response = await s.handler(
    viewerRequest({
      uri: '/_auth/callback',
      querystring: `code=abc&state=${state}`,
      cookies: { [login.name]: login.value },
    }),
  );
  assert.equal(response.status, '302');
  assert.equal(header(response, 'location')[0], `https://docs.galley.example.com/b/${HASH}`);
  assert.equal(tokenRequest.grant_type, 'authorization_code');
  assert.equal(tokenRequest.code, 'abc');
  assert.equal(tokenRequest.redirect_uri, 'https://galley.example.com/_auth/callback');
  assert.equal(tokenRequest.client_id, config.clientId);
  assert.ok(tokenRequest.code_verifier.length >= 43);
  assert.equal(tokenRequest.client_secret, undefined);

  const id = cookieNamed(response, ID_COOKIE);
  assert.deepEqual(id.attributes, ['Domain=galley.example.com', 'Path=/', 'Max-Age=2592000', 'Secure', 'HttpOnly', 'SameSite=Lax']);
  assert.equal(cookieNamed(response, REFRESH_COOKIE).value, REFRESH);
  const cleared = cookieNamed(response, login.name);
  assert.equal(cleared.value, '');
  assert.ok(cleared.attributes.includes('Max-Age=0'));
  assert.ok(cleared.attributes.includes('Path=/_auth/callback'));
});

test('戻り先は 2 つのホスト名だけに限る', async () => {
  const s = setup({
    token: () => jsonResponse(200, { id_token: idToken(s.key, s.time.seconds()), refresh_token: REFRESH }),
  });
  const state = 'A'.repeat(43);
  const cases = {
    'https://evil.example/': 'https://galley.example.com/',
    'http://galley.example.com/demo': 'https://galley.example.com/',
    'https://galley.example.com:8443/': 'https://galley.example.com/',
    'https://user@galley.example.com/': 'https://galley.example.com/',
    'https://galley.example.com.evil.example/': 'https://galley.example.com/',
    'https://galley.example.com/_auth/callback': 'https://galley.example.com/',
    'javascript:alert(1)': 'https://galley.example.com/',
    'https://docs.galley.example.com/b/x': 'https://docs.galley.example.com/b/x',
  };
  for (const [returnTo, expected] of Object.entries(cases)) {
    const saved = Buffer.from(JSON.stringify({ v: 'v'.repeat(43), r: returnTo })).toString('base64url');
    const response = await s.handler(
      viewerRequest({
        uri: '/_auth/callback',
        querystring: `code=abc&state=${state}`,
        cookies: { [`${LOGIN_COOKIE_PREFIX}${state}`]: saved },
      }),
    );
    assert.equal(header(response, 'location')[0], expected, returnTo);
  }
});

test('戻り先で state の Cookie が無い・合わないときは、トークンに換えずにやり直してもらう', async () => {
  const s = setup();
  const cases = [
    { querystring: `code=abc&state=${'A'.repeat(43)}`, cookies: {} },
    { querystring: `code=abc&state=${'A'.repeat(43)}`, cookies: { [`${LOGIN_COOKIE_PREFIX}${'B'.repeat(43)}`]: 'x' } },
    { querystring: 'code=abc&state=short', cookies: {} },
    { querystring: 'code=abc', cookies: {} },
  ];
  for (const c of cases) {
    const response = await s.handler(viewerRequest({ uri: '/_auth/callback', ...c }));
    assert.equal(response.status, '400');
    assert.match(response.body, /やり直/);
  }
  assert.equal(s.calls.filter((c) => c.url === tokenUrl).length, 0);
});

test('Cognito がエラーで戻したときや、コードを換えられないときはログインしない', async () => {
  const s = setup();
  const state = 'A'.repeat(43);
  const saved = Buffer.from(JSON.stringify({ v: 'v'.repeat(43), r: 'https://galley.example.com/' })).toString('base64url');
  for (const querystring of [`error=access_denied&state=${state}`, `code=used&state=${state}`]) {
    const response = await s.handler(
      viewerRequest({ uri: '/_auth/callback', querystring, cookies: { [`${LOGIN_COOKIE_PREFIX}${state}`]: saved } }),
    );
    assert.equal(response.status, '400');
    assert.equal(cookieNamed(response, ID_COOKIE), undefined);
  }
});

test('期限が近ければリフレッシュトークンで取り直し、Cookie を出し直して同じ URL に 307 で戻す', async () => {
  let tokenRequest;
  const s = setup({
    token: (body) => {
      tokenRequest = body;
      return jsonResponse(200, { id_token: idToken(s.key, s.time.seconds()), access_token: 'a', expires_in: 3600 });
    },
  });
  const token = loggedIn(s);
  s.time.advance(3600 - 120);
  const response = await s.handler(
    viewerRequest({
      host: config.docsHost,
      uri: `/b/${HASH}`,
      method: 'GET',
      cookies: { [ID_COOKIE]: token, [REFRESH_COOKIE]: REFRESH },
      headers: { 'sec-fetch-dest': 'iframe' },
    }),
  );
  assert.equal(response.status, '307');
  assert.equal(header(response, 'location')[0], `https://docs.galley.example.com/b/${HASH}`);
  assert.deepEqual(tokenRequest, { client_id: config.clientId, grant_type: 'refresh_token', refresh_token: REFRESH });
  assert.ok(cookieNamed(response, ID_COOKIE).value);
  // リフレッシュトークンが返らなければ（入れ替えなし）、今の Cookie のままにする
  assert.equal(cookieNamed(response, REFRESH_COOKIE), undefined);
});

test('期限切れで取り直せなければ、Cookie を消してログインに回す', async () => {
  const s = setup();
  const token = loggedIn(s);
  s.time.advance(7200);
  const response = await s.handler(viewerRequest({ uri: '/', cookies: { [ID_COOKIE]: token, [REFRESH_COOKIE]: REFRESH_2 } }));
  assert.equal(response.status, '302');
  assert.equal(cookieNamed(response, ID_COOKIE).value, '');
  assert.equal(cookieNamed(response, REFRESH_COOKIE).value, '');
});

test('期限が近くても、取り直せなければ期限まではそのまま通す', async () => {
  for (const token of [() => jsonResponse(400, { error: 'invalid_grant' }), () => jsonResponse(500, {})]) {
    const s = setup({ token });
    const id = loggedIn(s);
    s.time.advance(3600 - 120);
    const result = await s.handler(viewerRequest({ uri: '/', cookies: { [ID_COOKIE]: id, [REFRESH_COOKIE]: REFRESH } }));
    assert.ok(isRequest(result));
  }
});

test('JWKS を取れないときは、ログインに回さずに 503 を返す', async () => {
  const s = setup({
    jwks: () => {
      throw new TypeError('fetch failed');
    },
  });
  for (const event of [
    viewerRequest({ uri: '/', cookies: { [ID_COOKIE]: loggedIn(s) } }),
    viewerRequest({ host: config.docsHost, uri: `/b/${HASH}`, cookies: { [ID_COOKIE]: loggedIn(s) } }),
  ]) {
    const response = await s.handler(event);
    assert.equal(response.status, '503');
    assert.equal(header(response, 'location').length, 0);
    assert.equal(header(response, 'cache-control')[0], 'no-store');
  }
  const docs = await s.handler(viewerRequest({ host: config.docsHost, uri: `/b/${HASH}`, cookies: { [ID_COOKIE]: loggedIn(s) } }));
  assert.equal(header(docs, 'content-security-policy')[0], config.docsCsp);
});

test('Cognito のトークンのエンドポイントに届かず、ID トークンも切れていれば 503 を返す', async () => {
  const s = setup({
    token: () => {
      throw new TypeError('fetch failed');
    },
  });
  const token = loggedIn(s);
  s.time.advance(7200);
  const response = await s.handler(viewerRequest({ uri: '/', cookies: { [ID_COOKIE]: token, [REFRESH_COOKIE]: REFRESH } }));
  assert.equal(response.status, '503');
});

test('資料配信では、ログインを確かめてから /b/<hash> を S3 のキーに書き換える', async () => {
  const s = setup();
  const cookies = { [ID_COOKIE]: loggedIn(s) };
  const plain = await s.handler(viewerRequest({ host: config.docsHost, uri: `/b/${HASH}`, querystring: 'x=1', cookies }));
  assert.ok(isRequest(plain));
  assert.equal(plain.uri, `/blobs/e3/b0/${HASH}.html`);
  assert.equal(plain.querystring, '');

  const download = await s.handler(viewerRequest({ host: config.docsHost, uri: `/b/${HASH}`, querystring: 'download=keikaku&x=1', cookies }));
  assert.equal(download.querystring, 'download=keikaku');

  const bad = await s.handler(viewerRequest({ host: config.docsHost, uri: `/b/${HASH}`, querystring: 'download=Keikaku', cookies }));
  assert.equal(bad.status, '400');
  assert.equal(header(bad, 'content-security-policy')[0], config.docsCsp);

  for (const uri of ['/', `/b/${HASH.toUpperCase()}`, `/b/${HASH}/`, '/b/../secret', `/r/${HASH}`]) {
    const response = await s.handler(viewerRequest({ host: config.docsHost, uri, cookies }));
    assert.equal(response.status, '404', uri);
  }
});

test('ログインしていなければ、資料配信のパスを書き換えない', async () => {
  const s = setup();
  const response = await s.handler(viewerRequest({ host: config.docsHost, uri: `/b/${HASH}` }));
  assert.equal(response.status, '302');
});

test('知らないホスト名（CloudFront の既定のドメインなど）は断る', async () => {
  const s = setup();
  const response = await s.handler(viewerRequest({ host: 'd111111abcdef8.cloudfront.net', uri: '/' }));
  assert.equal(response.status, '404');
});

test('ログアウトで Cookie を消し、リフレッシュトークンを取り消して Cognito のログアウトに回す', async () => {
  const s = setup();
  const response = await s.handler(
    viewerRequest({ uri: '/_auth/signout', cookies: { [ID_COOKIE]: loggedIn(s), [REFRESH_COOKIE]: REFRESH } }),
  );
  assert.equal(response.status, '302');
  const location = new URL(header(response, 'location')[0]);
  assert.equal(location.pathname, '/logout');
  assert.equal(location.searchParams.get('logout_uri'), 'https://galley.example.com/');
  assert.equal(cookieNamed(response, ID_COOKIE).value, '');
  assert.equal(cookieNamed(response, REFRESH_COOKIE).value, '');
  const revoke = s.calls.find((c) => c.url === revokeUrl);
  assert.deepEqual(revoke.body, { client_id: config.clientId, token: REFRESH });
});

test('/_auth/ のほかのパスはアプリに渡さない', async () => {
  const s = setup();
  const response = await s.handler(viewerRequest({ uri: '/_auth/other', cookies: { [ID_COOKIE]: loggedIn(s) } }));
  assert.equal(response.status, '404');
});

test('リフレッシュトークンの形が合わない・ID トークンが無い・偽の ID トークンなら、Cognito を呼ばずにログインに回す', async () => {
  const s = setup({
    token: () => jsonResponse(200, { id_token: idToken(s.key, s.time.seconds()) }),
  });
  const now = s.time.seconds();
  const expired = idToken(s.key, now - 7200, { exp: now - 3600 });
  const cases = {
    'ID トークンなし': { [REFRESH_COOKIE]: REFRESH },
    '偽の ID トークン': { [ID_COOKIE]: idToken(signingKey('key-1'), now - 7200, { exp: now - 3600 }), [REFRESH_COOKIE]: REFRESH },
    '別のクライアントの ID トークン': { [ID_COOKIE]: idToken(s.key, now - 7200, { exp: now - 3600, aud: 'other' }), [REFRESH_COOKIE]: REFRESH },
    短すぎる: { [ID_COOKIE]: expired, [REFRESH_COOKIE]: 'short' },
    使えない文字: { [ID_COOKIE]: expired, [REFRESH_COOKIE]: `${REFRESH}%` },
    長すぎる: { [ID_COOKIE]: expired, [REFRESH_COOKIE]: 'a'.repeat(8193) },
  };
  for (const [name, cookies] of Object.entries(cases)) {
    const response = await s.handler(viewerRequest({ uri: '/', cookies }));
    assert.equal(response.status, '302', name);
    assert.match(header(response, 'location')[0], /\/oauth2\/authorize\?/, name);
    assert.equal(cookieNamed(response, REFRESH_COOKIE).value, '', name);
  }
  assert.equal(s.calls.filter((c) => c.url === tokenUrl).length, 0);

  // このプールが出した期限切れの ID トークンと、形の合うリフレッシュトークンなら取り直す
  const refreshed = await s.handler(viewerRequest({ uri: '/', cookies: { [ID_COOKIE]: expired, [REFRESH_COOKIE]: REFRESH } }));
  assert.equal(refreshed.status, '307');
  assert.equal(s.calls.filter((c) => c.url === tokenUrl).length, 1);
});

test('取り直すまでの残り時間は、設定（有効期間から決めた値）に従う', async () => {
  const time = clock();
  const key = signingKey('key-1');
  const { fetch, calls } = fakeFetch({
    [jwksUrl]: () => jsonResponse(200, { keys: [key.jwk] }),
    [tokenUrl]: () => jsonResponse(200, { id_token: idToken(key, time.seconds(), { exp: time.seconds() + 300 }) }),
  });
  const handler = createHandler({ ...config, refreshBeforeSeconds: 75 }, { fetch, now: time.now });
  const token = idToken(key, time.seconds(), { exp: time.seconds() + 300 });
  const cookies = { [ID_COOKIE]: token, [REFRESH_COOKIE]: REFRESH };
  time.advance(200);
  assert.ok(isRequest(await handler(viewerRequest({ uri: '/', cookies }))));
  time.advance(30);
  assert.equal((await handler(viewerRequest({ uri: '/', cookies }))).status, '307');
  assert.equal(calls.filter((c) => c.url === tokenUrl).length, 1);
});

test('戻り先で、途中でやめたログインの Cookie もまとめて消す', async () => {
  const s = setup({
    token: () => jsonResponse(200, { id_token: idToken(s.key, s.time.seconds()), refresh_token: REFRESH }),
  });
  const state = 'A'.repeat(43);
  const saved = Buffer.from(JSON.stringify({ v: 'v'.repeat(43), r: 'https://galley.example.com/' })).toString('base64url');
  const response = await s.handler(
    viewerRequest({
      uri: '/_auth/callback',
      querystring: `code=abc&state=${state}`,
      cookies: {
        [`${LOGIN_COOKIE_PREFIX}${state}`]: saved,
        [`${LOGIN_COOKIE_PREFIX}${'B'.repeat(43)}`]: 'old',
        [`${LOGIN_COOKIE_PREFIX}${'C'.repeat(43)}`]: 'old',
        galley_author: 'x',
      },
    }),
  );
  assert.equal(response.status, '302');
  const cleared = setCookies(response)
    .filter((c) => c.name.startsWith(LOGIN_COOKIE_PREFIX))
    .map((c) => `${c.name}=${c.value}; ${c.attributes.includes('Max-Age=0')}`)
    .sort();
  assert.deepEqual(cleared, [
    `${LOGIN_COOKIE_PREFIX}${'A'.repeat(43)}=; true`,
    `${LOGIN_COOKIE_PREFIX}${'B'.repeat(43)}=; true`,
    `${LOGIN_COOKIE_PREFIX}${'C'.repeat(43)}=; true`,
  ]);
  assert.equal(cookieNamed(response, 'galley_author'), undefined);
});

test('ほかのサイトや資料の iframe から移動させられたログアウトは、すぐには行わずリンクを出す', async () => {
  const s = setup();
  for (const site of ['cross-site', 'same-site']) {
    const response = await s.handler(
      viewerRequest({
        uri: '/_auth/signout',
        cookies: { [ID_COOKIE]: loggedIn(s), [REFRESH_COOKIE]: REFRESH },
        headers: { 'sec-fetch-site': site },
      }),
    );
    assert.equal(response.status, '200', site);
    assert.match(response.body, /href="https:\/\/galley\.example\.com\/_auth\/signout"/);
    assert.deepEqual(setCookies(response), [], site);
    assert.ok(header(response, 'content-security-policy')[0].includes("frame-ancestors 'none'"));
  }
  assert.equal(s.calls.filter((c) => c.url === revokeUrl).length, 0);

  for (const site of ['same-origin', 'none']) {
    const response = await s.handler(
      viewerRequest({ uri: '/_auth/signout', cookies: { [REFRESH_COOKIE]: REFRESH }, headers: { 'sec-fetch-site': site } }),
    );
    assert.equal(response.status, '302', site);
    assert.equal(cookieNamed(response, ID_COOKIE).value, '', site);
  }
});

test('origin-response: 200 には長いキャッシュと、download があれば Content-Disposition を付ける', async () => {
  const s = setup();
  const uri = `/blobs/e3/b0/${HASH}.html`;
  const plain = await s.handler(originResponse({ uri, status: 200, headers: { 'content-type': 'text/html; charset=utf-8' } }));
  // ログインが要る内容なので、途中のキャッシュには置かせない（CloudFront はキャッシュポリシーの最小 TTL でキャッシュする）
  assert.equal(header(plain, 'cache-control')[0], 'private, max-age=31536000, immutable');
  assert.deepEqual(header(plain, 'content-disposition'), []);

  const download = await s.handler(originResponse({ uri, querystring: 'download=keikaku', status: 200 }));
  assert.equal(header(download, 'content-disposition')[0], 'attachment; filename="keikaku.html"');

  const range = await s.handler(originResponse({ uri, querystring: 'download=keikaku', status: 206 }));
  assert.equal(header(range, 'content-disposition')[0], 'attachment; filename="keikaku.html"');

  const notModified = await s.handler(originResponse({ uri, status: 304 }));
  assert.equal(notModified.status, '304');
  assert.equal(notModified.body, undefined);
});

test('origin-response: S3 のエラーを素のテキストにし、キャッシュさせない', async () => {
  const s = setup();
  const uri = `/blobs/e3/b0/${HASH}.html`;
  const missing = await s.handler(
    originResponse({ uri, status: 404, headers: { 'content-type': 'application/xml', 'content-length': '243', via: '1.1 x' } }),
  );
  assert.equal(missing.status, '404');
  assert.equal(missing.body, '資料が見つかりません\n');
  assert.equal(header(missing, 'content-type')[0], 'text/plain; charset=utf-8');
  assert.equal(header(missing, 'cache-control')[0], 'no-store');
  assert.equal(header(missing, 'content-security-policy')[0], config.docsCsp);
  assert.deepEqual(header(missing, 'content-length'), []);
  assert.deepEqual(header(missing, 'via'), ['1.1 x']);

  const denied = await s.handler(originResponse({ uri, status: 403 }));
  assert.equal(denied.status, '403');
  assert.equal(denied.body, '資料を表示できません\n');
});
