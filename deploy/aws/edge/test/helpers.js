'use strict';

const crypto = require('node:crypto');

const config = {
  region: 'ap-northeast-1',
  userPoolId: 'ap-northeast-1_TEST',
  clientId: 'browser-client',
  cognitoDomain: 'galley-test.auth.ap-northeast-1.amazoncognito.com',
  appHost: 'galley.example.com',
  docsHost: 'docs.galley.example.com',
  cookieDomain: 'galley.example.com',
  blobPrefix: 'blobs',
  scopes: 'openid email profile',
  sessionMaxAgeSeconds: 2592000,
  docsCsp: "sandbox allow-scripts; default-src 'none'",
};

const issuer = `https://cognito-idp.${config.region}.amazonaws.com/${config.userPoolId}`;
const jwksUrl = `${issuer}/.well-known/jwks.json`;
const tokenUrl = `https://${config.cognitoDomain}/oauth2/token`;
const revokeUrl = `https://${config.cognitoDomain}/oauth2/revoke`;

/** テスト用の時計。秒で進める。 */
function clock(start = 1_800_000_000) {
  let ms = start * 1000;
  return {
    now: () => ms,
    seconds: () => Math.floor(ms / 1000),
    advance(seconds) {
      ms += seconds * 1000;
    },
  };
}

function signingKey(kid) {
  const { privateKey, publicKey } = crypto.generateKeyPairSync('rsa', { modulusLength: 2048 });
  return { kid, privateKey, jwk: { ...publicKey.export({ format: 'jwk' }), kid, alg: 'RS256', use: 'sig' } };
}

function sign(key, claims, header = {}) {
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString('base64url');
  const input = `${encode({ alg: 'RS256', kid: key.kid, ...header })}.${encode(claims)}`;
  const signature = crypto.sign('RSA-SHA256', Buffer.from(input), key.privateKey).toString('base64url');
  return `${input}.${signature}`;
}

function idToken(key, now, overrides = {}) {
  return sign(key, {
    iss: issuer,
    aud: config.clientId,
    token_use: 'id',
    sub: 'user-1',
    email: 'user@example.com',
    iat: now,
    exp: now + 3600,
    ...overrides,
  });
}

function jsonResponse(status, body) {
  return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
}

/**
 * Cognito を真似る fetch。`routes` の URL ごとに、応答を返す関数を置く。呼ばれた記録を `calls` に残す。
 */
function fakeFetch(routes) {
  const calls = [];
  const fetch = async (url, init = {}) => {
    const body = init.body ? Object.fromEntries(new URLSearchParams(init.body)) : undefined;
    calls.push({ url, method: init.method || 'GET', body });
    const route = routes[url];
    if (!route) {
      throw new Error(`想定しない URL: ${url}`);
    }
    return route(body);
  };
  return { fetch, calls };
}

function viewerRequest({ host = config.appHost, uri = '/', querystring = '', method = 'GET', cookies = {}, headers = {} } = {}) {
  const cfHeaders = { host: [{ key: 'Host', value: host }] };
  const cookieValue = Object.entries(cookies)
    .map(([name, value]) => `${name}=${value}`)
    .join('; ');
  if (cookieValue) {
    cfHeaders.cookie = [{ key: 'Cookie', value: cookieValue }];
  }
  for (const [name, value] of Object.entries(headers)) {
    cfHeaders[name] = [{ key: name, value }];
  }
  return {
    Records: [{ cf: { config: { eventType: 'viewer-request' }, request: { method, uri, querystring, headers: cfHeaders } } }],
  };
}

function originResponse({ uri, querystring = '', status, headers = {} }) {
  const cfHeaders = {};
  for (const [name, value] of Object.entries(headers)) {
    cfHeaders[name] = [{ key: name, value }];
  }
  return {
    Records: [
      {
        cf: {
          config: { eventType: 'origin-response' },
          request: { method: 'GET', uri, querystring, headers: {} },
          response: { status: String(status), statusDescription: '', headers: cfHeaders },
        },
      },
    ],
  };
}

function header(response, name) {
  const values = response.headers && response.headers[name];
  return values ? values.map((h) => h.value) : [];
}

/** Set-Cookie から、名前と値・属性を取り出す。 */
function setCookies(response) {
  return header(response, 'set-cookie').map((value) => {
    const [pair, ...attributes] = value.split('; ');
    const index = pair.indexOf('=');
    return { name: pair.slice(0, index), value: pair.slice(index + 1), attributes };
  });
}

module.exports = {
  config,
  issuer,
  jwksUrl,
  tokenUrl,
  revokeUrl,
  clock,
  signingKey,
  sign,
  idToken,
  jsonResponse,
  fakeFetch,
  viewerRequest,
  originResponse,
  header,
  setCookies,
};
