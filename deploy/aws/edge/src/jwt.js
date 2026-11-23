'use strict';

const crypto = require('node:crypto');

const { ServiceUnavailableError } = require('./errors');

/** 知らない kid のトークンが来ても、JWKS を取り直すのはこの間隔に 1 回まで（偽のトークンで Cognito を叩かせない）。 */
const JWKS_MIN_REFETCH_MS = 60_000;
/**
 * Cognito を呼ぶときのタイムアウト。戻り先ではトークンの交換と JWKS の取得が続くことがあるので、
 * 2 回分でも関数のタイムアウト（5 秒）に収める。
 */
const FETCH_TIMEOUT_MS = 2_000;
/** `iat` が未来を指していても許す幅（時計のずれ）。 */
const CLOCK_SKEW_SECONDS = 60;

function decodeJson(segment) {
  try {
    const value = JSON.parse(Buffer.from(segment, 'base64url').toString('utf8'));
    return value !== null && typeof value === 'object' ? value : null;
  } catch {
    return null;
  }
}

/**
 * Cognito の ID トークン（RS256）を確かめる。
 *
 * 公開鍵（JWKS）は関数のグローバルな変数（この関数が返すオブジェクト）に持ち、実行環境が続く間は使い回す。
 * 鍵の入れ替えに備えて、トークンの kid が手元の鍵に無いときだけ取り直す。
 */
function createVerifier({ issuer, clientId, fetch, now }) {
  const jwksUrl = `${issuer}/.well-known/jwks.json`;
  let keys = new Map();
  let fetchedAt = Number.NEGATIVE_INFINITY;
  let lastFetchFailed = false;

  async function refreshKeys() {
    fetchedAt = now();
    let body;
    try {
      const response = await fetch(jwksUrl, { signal: AbortSignal.timeout(FETCH_TIMEOUT_MS) });
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      body = await response.json();
    } catch (err) {
      lastFetchFailed = true;
      throw new ServiceUnavailableError(`JWKS を取得できません: ${err.message}`);
    }
    lastFetchFailed = false;
    const next = new Map();
    for (const jwk of Array.isArray(body && body.keys) ? body.keys : []) {
      if (jwk.kty !== 'RSA' || typeof jwk.kid !== 'string' || (jwk.use && jwk.use !== 'sig')) {
        continue;
      }
      try {
        next.set(jwk.kid, crypto.createPublicKey({ key: jwk, format: 'jwk' }));
      } catch {
        // 読めない鍵は飛ばす
      }
    }
    keys = next;
  }

  async function keyFor(kid) {
    if (keys.has(kid)) {
      return keys.get(kid);
    }
    if (now() - fetchedAt < JWKS_MIN_REFETCH_MS) {
      // 直前に取れなかったのなら、鍵が無いのはトークンのせいとは限らない。ログインに回さず 503 にする
      if (lastFetchFailed) {
        throw new ServiceUnavailableError('JWKS を取得できません（直前の取得に失敗）');
      }
      return null;
    }
    await refreshKeys();
    return keys.get(kid) || null;
  }

  /**
   * 有効な ID トークンなら中身（claims）を、そうでなければ null を返す。
   * JWKS を取れないときは ServiceUnavailableError を投げる。
   *
   * `allowExpired` を付けると、期限だけは見ない（署名・発行元・aud・種類は確かめる）。リフレッシュトークンで
   * 取り直す前に、このプールが出したトークンを持っていることを確かめるために使う。
   */
  async function verifyIdToken(token, { allowExpired = false } = {}) {
    if (typeof token !== 'string') {
      return null;
    }
    const parts = token.split('.');
    if (parts.length !== 3) {
      return null;
    }
    const [encodedHeader, encodedPayload, encodedSignature] = parts;
    const header = decodeJson(encodedHeader);
    const claims = decodeJson(encodedPayload);
    if (!header || !claims || header.alg !== 'RS256' || typeof header.kid !== 'string') {
      return null;
    }
    // 署名より先に安い検査をして、期限切れや別のプールのトークンで JWKS を取りに行かない
    const nowSeconds = Math.floor(now() / 1000);
    if (
      claims.iss !== issuer ||
      claims.token_use !== 'id' ||
      claims.aud !== clientId ||
      typeof claims.exp !== 'number' ||
      (!allowExpired && claims.exp <= nowSeconds) ||
      (typeof claims.iat === 'number' && claims.iat > nowSeconds + CLOCK_SKEW_SECONDS)
    ) {
      return null;
    }
    const key = await keyFor(header.kid);
    if (!key) {
      return null;
    }
    const valid = crypto.verify(
      'RSA-SHA256',
      Buffer.from(`${encodedHeader}.${encodedPayload}`),
      key,
      Buffer.from(encodedSignature, 'base64url'),
    );
    return valid ? claims : null;
  }

  return { verifyIdToken };
}

module.exports = { createVerifier, FETCH_TIMEOUT_MS, JWKS_MIN_REFETCH_MS };
