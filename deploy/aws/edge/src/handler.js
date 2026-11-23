'use strict';

const { createCognito, randomToken, codeChallenge } = require('./cognito');
const { parseCookies, serializeCookie } = require('./cookies');
const { rewriteBlobRequest, finishBlobResponse } = require('./docs');
const { ServiceUnavailableError } = require('./errors');
const { createVerifier } = require('./jwt');
const { createResponses } = require('./responses');

const ID_COOKIE = 'galley_id_token';
const REFRESH_COOKIE = 'galley_refresh_token';
/** ログインを始めるときの PKCE の code_verifier と戻り先。state ごとに分け、複数のタブで同時にログインしても混ざらない。 */
const LOGIN_COOKIE_PREFIX = 'galley_login_';
const LOGIN_COOKIE_MAX_AGE = 600;
const AUTH_PREFIX = '/_auth/';
const CALLBACK_PATH = '/_auth/callback';
const SIGNOUT_PATH = '/_auth/signout';
const STATE_PATTERN = /^[A-Za-z0-9_-]{22,64}$/;
/**
 * リフレッシュトークンとして受け付ける形（文字の種類と長さ）。Cognito のリフレッシュトークンは中身を見られない
 * （暗号化されている）ので、形だけを見て、明らかに違う値で Cognito を呼ばない。
 */
const REFRESH_TOKEN_PATTERN = /^[A-Za-z0-9._-]{20,8192}$/;
/** ログアウトを受け付けるリクエスト元。ほかのサイトや資料の iframe から移動させられたものは、確かめてから行う。 */
const SIGNOUT_FETCH_SITES = new Set(['', 'same-origin', 'none']);

function headerValue(request, name) {
  const values = request.headers[name];
  return values && values.length > 0 ? values[0].value : '';
}

function encodeLoginState(state) {
  return Buffer.from(JSON.stringify(state)).toString('base64url');
}

function decodeLoginState(value) {
  if (!value) {
    return null;
  }
  try {
    const state = JSON.parse(Buffer.from(value, 'base64url').toString('utf8'));
    return state && typeof state.v === 'string' && typeof state.r === 'string' ? state : null;
  } catch {
    return null;
  }
}

/**
 * CloudFront の Lambda@Edge。2 つの CloudFront（画面と資料配信）に同じ関数を付ける。
 *
 * - viewer-request: Cognito のログインを確かめる。画面のホストでは `/_auth/callback`・`/_auth/signout` に答え、
 *   資料配信のホストでは `/b/<hash>` を S3 のキーに書き換える
 * - origin-response（資料配信だけ）: キャッシュの指定とダウンロードの `Content-Disposition` を付ける
 *
 * `config` は Terraform が作る（Lambda@Edge は環境変数を使えないため）。`deps` はテストで差し替える。
 */
function createHandler(config, deps = {}) {
  const fetchImpl = deps.fetch || globalThis.fetch;
  const now = deps.now || Date.now;
  const appOrigin = `https://${config.appHost}`;
  const issuer = `https://cognito-idp.${config.region}.amazonaws.com/${config.userPoolId}`;
  const verifier = createVerifier({ issuer, clientId: config.clientId, fetch: fetchImpl, now });
  const cognito = createCognito({
    domain: config.cognitoDomain,
    clientId: config.clientId,
    redirectUri: `${appOrigin}${CALLBACK_PATH}`,
    scopes: config.scopes,
    fetch: fetchImpl,
  });
  const responses = createResponses(config);
  /** ID トークンの残りがこれより短ければ、リフレッシュトークンで取り直す（Terraform が有効期間から決める）。 */
  const refreshBeforeSeconds = config.refreshBeforeSeconds;

  function cookie(name, value, { path = '/', maxAge = config.sessionMaxAgeSeconds } = {}) {
    return serializeCookie(name, value, { domain: config.cookieDomain, path, maxAge });
  }

  function clearCookie(name, path = '/') {
    return cookie(name, '', { path, maxAge: 0 });
  }

  function siteOf(request) {
    const host = headerValue(request, 'host').toLowerCase();
    if (host === config.appHost) {
      return 'app';
    }
    return host === config.docsHost ? 'docs' : null;
  }

  function currentUrl(request) {
    const host = request.headers.host[0].value.toLowerCase();
    return `https://${host}${request.uri}${request.querystring ? `?${request.querystring}` : ''}`;
  }

  /** 戻り先は 2 つのホスト名の https の URL だけ。ほかは画面のトップにする。 */
  function safeReturnUrl(value) {
    try {
      const url = new URL(value);
      const allowed =
        url.protocol === 'https:' &&
        (url.hostname === config.appHost || url.hostname === config.docsHost) &&
        url.port === '' &&
        url.username === '' &&
        url.password === '' &&
        !url.pathname.startsWith(AUTH_PREFIX);
      return allowed ? url.href : `${appOrigin}/`;
    } catch {
      return `${appOrigin}/`;
    }
  }

  function startLogin(request, site, setCookies) {
    const state = randomToken();
    const codeVerifier = randomToken();
    const loginCookie = cookie(`${LOGIN_COOKIE_PREFIX}${state}`, encodeLoginState({ v: codeVerifier, r: currentUrl(request) }), {
      path: CALLBACK_PATH,
      maxAge: LOGIN_COOKIE_MAX_AGE,
    });
    const location = cognito.authorizeUrl({ state, challenge: codeChallenge(codeVerifier) });
    return responses.redirect(site, 302, location, [...setCookies, loginCookie]);
  }

  /**
   * ログインしていないとき。ログインの画面に回すのは、ページを開く（GET の移動）ときだけにする。
   * iframe の中（資料の表示）はログインの画面に回さず、開き直してもらう（PR #75「ログインの流れ」の 5）。
   */
  function unauthenticated(request, site, setCookies) {
    const dest = headerValue(request, 'sec-fetch-dest');
    const mode = headerValue(request, 'sec-fetch-mode');
    if (dest === 'iframe' || dest === 'frame') {
      return responses.page(site, 401, 'ログインし直してください', 'ログインの有効期限が切れました。画面を開き直すと、ログインの画面に移ります。', {
        setCookies,
      });
    }
    const isNavigation = (request.method === 'GET' || request.method === 'HEAD') && (mode === '' || mode === 'navigate');
    if (isNavigation) {
      return startLogin(request, site, setCookies);
    }
    return responses.text(site, 401, 'ログインが必要です。画面を開き直してください。', setCookies);
  }

  /** ログインを確かめる。通してよければ null、そうでなければ返す応答。 */
  async function authenticate(request, site) {
    const cookies = parseCookies(request.headers);
    const claims = await verifier.verifyIdToken(cookies.get(ID_COOKIE));
    const nowSeconds = Math.floor(now() / 1000);
    if (claims && claims.exp - nowSeconds > refreshBeforeSeconds) {
      return null;
    }
    // Cognito を呼ぶのは、リフレッシュトークンの形が合い、このプールが出した ID トークン（期限切れでよい）も
    // 持っているときだけにする。偽の Cookie を付けた匿名のリクエストで、トークンのエンドポイントを叩かせない
    const refreshToken = cookies.get(REFRESH_COOKIE);
    const canRefresh =
      typeof refreshToken === 'string' &&
      REFRESH_TOKEN_PATTERN.test(refreshToken) &&
      (claims !== null || (await verifier.verifyIdToken(cookies.get(ID_COOKIE), { allowExpired: true })) !== null);
    if (!canRefresh) {
      if (claims) {
        return null;
      }
      const stale = [ID_COOKIE, REFRESH_COOKIE].filter((name) => cookies.has(name)).map((name) => clearCookie(name));
      return unauthenticated(request, site, stale);
    }
    let tokens;
    try {
      tokens = await cognito.refresh(refreshToken);
    } catch (err) {
      // まだ期限内なら、取り直せなくても通す
      if (claims && err instanceof ServiceUnavailableError) {
        console.warn(err.message);
        return null;
      }
      throw err;
    }
    if (tokens && (await verifier.verifyIdToken(tokens.id_token))) {
      // Cookie を出し直すために、同じ URL に戻す。307 なので POST も本文ごと送り直される
      const setCookies = [cookie(ID_COOKIE, tokens.id_token)];
      if (typeof tokens.refresh_token === 'string') {
        setCookies.push(cookie(REFRESH_COOKIE, tokens.refresh_token));
      }
      return responses.redirect(site, 307, currentUrl(request), setCookies);
    }
    if (claims) {
      return null;
    }
    return unauthenticated(request, site, [clearCookie(ID_COOKIE), clearCookie(REFRESH_COOKIE)]);
  }

  /** Cognito から戻ってきたところ。コードをトークンに換え、Cookie を出して元の URL に戻す。 */
  async function callback(request) {
    const retry = { href: `${appOrigin}/`, label: '最初からやり直す' };
    const params = new URLSearchParams(request.querystring);
    const state = params.get('state') || '';
    if (!STATE_PATTERN.test(state)) {
      return responses.page('app', 400, 'ログインをやり直してください', 'ログインの手続きが正しくありません。', { link: retry });
    }
    const loginCookieName = `${LOGIN_COOKIE_PREFIX}${state}`;
    const cookies = parseCookies(request.headers);
    // 戻り先には、途中でやめたログインの Cookie も届く（Path が同じ）。ここでまとめて消し、溜まらないようにする
    const loginCookies = new Set([loginCookieName]);
    for (const name of cookies.keys()) {
      if (name.startsWith(LOGIN_COOKIE_PREFIX) && STATE_PATTERN.test(name.slice(LOGIN_COOKIE_PREFIX.length))) {
        loginCookies.add(name);
      }
    }
    const setCookies = [...loginCookies].map((name) => clearCookie(name, CALLBACK_PATH));
    const saved = decodeLoginState(cookies.get(loginCookieName));
    if (!saved) {
      // 期限（10 分）を過ぎたか、別のブラウザーで戻ってきた。ここで自動でやり直すと、Cookie を保存できない
      // ブラウザーで行き来が止まらなくなるので、リンクを出す
      return responses.page('app', 400, 'ログインをやり直してください', 'ログインの手続きの期限が切れました。', {
        setCookies,
        link: retry,
      });
    }
    const code = params.get('code');
    if (params.has('error') || !code) {
      return responses.page('app', 400, 'ログインできませんでした', 'ログインの画面から戻ってきましたが、ログインは完了していません。', {
        setCookies,
        link: retry,
      });
    }
    const tokens = await cognito.exchangeCode(code, saved.v);
    if (!tokens || !(await verifier.verifyIdToken(tokens.id_token))) {
      return responses.page('app', 400, 'ログインをやり直してください', 'ログインを確かめられませんでした。', { setCookies, link: retry });
    }
    setCookies.push(cookie(ID_COOKIE, tokens.id_token));
    if (typeof tokens.refresh_token === 'string') {
      setCookies.push(cookie(REFRESH_COOKIE, tokens.refresh_token));
    }
    return responses.redirect('app', 302, safeReturnUrl(saved.r), setCookies);
  }

  /**
   * Cookie を消し、リフレッシュトークンを取り消して、Cognito のログアウトに回す。
   * ほかのサイトや資料の iframe から移動させられたとき（Sec-Fetch-Site が same-origin・none 以外）は、
   * すぐには行わず、ログアウトのリンクを出して利用者に押してもらう
   */
  async function signout(request) {
    if (!SIGNOUT_FETCH_SITES.has(headerValue(request, 'sec-fetch-site'))) {
      return responses.page('app', 200, 'ログアウトしますか', 'ほかのページからログアウトに移動しました。ログアウトするなら、下のリンクを押してください。', {
        link: { href: `${appOrigin}${SIGNOUT_PATH}`, label: 'ログアウトする' },
      });
    }
    const refreshToken = parseCookies(request.headers).get(REFRESH_COOKIE);
    if (refreshToken) {
      await cognito.revoke(refreshToken);
    }
    return responses.redirect('app', 302, cognito.logoutUrl(`${appOrigin}/`), [
      clearCookie(ID_COOKIE),
      clearCookie(REFRESH_COOKIE),
    ]);
  }

  function routeDocs(request) {
    const rewritten = rewriteBlobRequest(request.uri, request.querystring, config.blobPrefix);
    if (rewritten.status === 404) {
      return responses.text('docs', 404, '資料が見つかりません');
    }
    if (rewritten.status === 400) {
      return responses.text('docs', 400, 'ダウンロードのファイル名が正しくありません');
    }
    request.uri = rewritten.uri;
    request.querystring = rewritten.querystring;
    return request;
  }

  async function viewerRequest(request) {
    const site = siteOf(request);
    if (!site) {
      // CloudFront の既定のドメイン（*.cloudfront.net）など。ログインの Cookie も届かないので断る
      return responses.text('docs', 404, '見つかりません');
    }
    try {
      if (site === 'app' && request.uri.startsWith(AUTH_PREFIX)) {
        if (request.uri === CALLBACK_PATH) {
          return await callback(request);
        }
        if (request.uri === SIGNOUT_PATH) {
          return await signout(request);
        }
        return responses.text('app', 404, '見つかりません');
      }
      const denied = await authenticate(request, site);
      if (denied) {
        return denied;
      }
      return site === 'docs' ? routeDocs(request) : request;
    } catch (err) {
      if (err instanceof ServiceUnavailableError) {
        console.error(err.message);
        return responses.page(site, 503, 'ログインを確かめられません', '認証のサービスに接続できません。時間をおいて開き直してください。');
      }
      throw err;
    }
  }

  return async function handler(event) {
    const { config: eventConfig, request, response } = event.Records[0].cf;
    if (eventConfig.eventType === 'origin-response') {
      return finishBlobResponse(request, response, responses.securityHeaders('docs'));
    }
    return viewerRequest(request);
  };
}

module.exports = { createHandler, ID_COOKIE, REFRESH_COOKIE, LOGIN_COOKIE_PREFIX };
