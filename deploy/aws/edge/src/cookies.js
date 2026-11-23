'use strict';

/**
 * CloudFront の形の Cookie ヘッダー（`headers.cookie = [{ key, value }]`）を、名前から値への Map にする。
 * 同じ名前が何度もあれば、最初のものを使う。値はデコードしない（ここで扱うトークンは URL で安全な文字だけ）。
 */
function parseCookies(headers) {
  const cookies = new Map();
  for (const { value } of headers.cookie || []) {
    for (const part of value.split(';')) {
      const index = part.indexOf('=');
      if (index <= 0) {
        continue;
      }
      const name = part.slice(0, index).trim();
      if (name && !cookies.has(name)) {
        cookies.set(name, part.slice(index + 1).trim());
      }
    }
  }
  return cookies;
}

/**
 * ログインの Cookie。画面と資料配信の両方に届くよう、親のドメインを付ける。
 * `__Host-` の接頭辞は Domain と一緒に使えないので付けない。
 */
function serializeCookie(name, value, { domain, path, maxAge }) {
  return [
    `${name}=${value}`,
    `Domain=${domain}`,
    `Path=${path}`,
    `Max-Age=${maxAge}`,
    'Secure',
    'HttpOnly',
    'SameSite=Lax',
  ].join('; ');
}

module.exports = { parseCookies, serializeCookie };
