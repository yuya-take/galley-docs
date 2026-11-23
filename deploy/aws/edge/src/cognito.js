'use strict';

const crypto = require('node:crypto');

const { ServiceUnavailableError } = require('./errors');
const { FETCH_TIMEOUT_MS } = require('./jwt');

/** URL で安全な乱数の文字列（state・PKCE の code_verifier）。 */
function randomToken(bytes = 32) {
  return crypto.randomBytes(bytes).toString('base64url');
}

/** PKCE の code_challenge（S256）。 */
function codeChallenge(verifier) {
  return crypto.createHash('sha256').update(verifier).digest('base64url');
}

/**
 * Cognito のマネージドログインのエンドポイント。アプリクライアントは公開クライアント（シークレットなし）なので、
 * 関数に秘密は持たせない。
 */
function createCognito({ domain, clientId, redirectUri, scopes, fetch }) {
  const base = `https://${domain}`;

  async function post(path, params) {
    try {
      return await fetch(`${base}${path}`, {
        method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ client_id: clientId, ...params }).toString(),
        signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
      });
    } catch (err) {
      throw new ServiceUnavailableError(`Cognito（${path}）に届きません: ${err.message}`);
    }
  }

  /**
   * トークンのエンドポイントを呼ぶ。断られた（400: invalid_grant など）ときは null を返す。
   * それ以外の失敗は ServiceUnavailableError にする。
   */
  async function token(params) {
    const response = await post('/oauth2/token', params);
    if (response.status === 400) {
      return null;
    }
    if (!response.ok) {
      throw new ServiceUnavailableError(`Cognito のトークンのエンドポイントが HTTP ${response.status} を返しました`);
    }
    const body = await response.json().catch(() => null);
    if (!body || typeof body.id_token !== 'string') {
      throw new ServiceUnavailableError('Cognito の応答に ID トークンがありません');
    }
    return body;
  }

  return {
    authorizeUrl({ state, challenge }) {
      const query = new URLSearchParams({
        response_type: 'code',
        client_id: clientId,
        redirect_uri: redirectUri,
        scope: scopes,
        state,
        code_challenge: challenge,
        code_challenge_method: 'S256',
      });
      return `${base}/oauth2/authorize?${query}`;
    },

    exchangeCode(code, verifier) {
      return token({
        grant_type: 'authorization_code',
        code,
        redirect_uri: redirectUri,
        code_verifier: verifier,
      });
    },

    refresh(refreshToken) {
      return token({ grant_type: 'refresh_token', refresh_token: refreshToken });
    },

    /** リフレッシュトークンを取り消す。ログアウトのときに使い、失敗しても止めない。 */
    async revoke(refreshToken) {
      try {
        await post('/oauth2/revoke', { token: refreshToken });
      } catch (err) {
        console.warn(err.message);
      }
    },

    logoutUrl(logoutUri) {
      return `${base}/logout?${new URLSearchParams({ client_id: clientId, logout_uri: logoutUri })}`;
    },
  };
}

module.exports = { createCognito, randomToken, codeChallenge };
