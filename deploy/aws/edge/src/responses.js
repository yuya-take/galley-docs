'use strict';

/** 画面のホストで関数が返す短いページの CSP。スクリプトは使わない。 */
const PAGE_CSP =
  "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

const STATUS_TEXT = {
  200: 'OK',
  302: 'Found',
  307: 'Temporary Redirect',
  400: 'Bad Request',
  401: 'Unauthorized',
  403: 'Forbidden',
  404: 'Not Found',
  502: 'Bad Gateway',
  503: 'Service Unavailable',
};

/** `content-security-policy` → `Content-Security-Policy` */
function canonicalName(name) {
  return name
    .split('-')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join('-');
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

/** `{ name: value }` を CloudFront のヘッダーの形（`{ name: [{ key, value }] }`）にする。 */
function toCloudFrontHeaders(headers, setCookies = []) {
  const result = {};
  for (const [name, value] of Object.entries(headers)) {
    result[name] = [{ key: canonicalName(name), value }];
  }
  if (setCookies.length > 0) {
    result['set-cookie'] = setCookies.map((value) => ({ key: 'Set-Cookie', value }));
  }
  return result;
}

/**
 * 関数が自分で返す応答。応答ヘッダーポリシーがこの応答にも効くかはドキュメントに書かれていないので、
 * 資料配信では CSP の sandbox などを関数でも付ける（PR #75「ログインの Cookie が資料配信に届くことの安全性」）。
 */
function createResponses({ docsCsp }) {
  function securityHeaders(site) {
    return {
      'cache-control': 'no-store',
      'content-security-policy': site === 'docs' ? docsCsp : PAGE_CSP,
      'x-content-type-options': 'nosniff',
      'referrer-policy': 'no-referrer',
    };
  }

  function build(site, status, headers, body, setCookies) {
    const response = {
      status: String(status),
      statusDescription: STATUS_TEXT[status] || '',
      headers: toCloudFrontHeaders({ ...securityHeaders(site), ...headers }, setCookies),
    };
    if (body !== undefined) {
      response.body = body;
      response.bodyEncoding = 'text';
    }
    return response;
  }

  return {
    securityHeaders,

    redirect(site, status, location, setCookies = []) {
      return build(site, status, { location }, undefined, setCookies);
    },

    text(site, status, message, setCookies = []) {
      return build(site, status, { 'content-type': 'text/plain; charset=utf-8' }, `${message}\n`, setCookies);
    },

    /** 人が読む短いページ。`link` があれば、そこへのリンクを付ける。 */
    page(site, status, title, message, { setCookies = [], link } = {}) {
      const anchor = link ? `<p><a href="${escapeHtml(link.href)}">${escapeHtml(link.label)}</a></p>` : '';
      const body = [
        '<!doctype html>',
        '<html lang="ja"><head><meta charset="utf-8">',
        '<meta name="viewport" content="width=device-width, initial-scale=1">',
        `<title>${escapeHtml(title)} - Galley</title>`,
        '<style>body{font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem;line-height:1.7;color:#222}</style>',
        '</head><body>',
        `<h1 style="font-size:1.25rem">${escapeHtml(title)}</h1>`,
        `<p>${escapeHtml(message)}</p>`,
        anchor,
        '</body></html>',
      ].join('');
      return build(site, status, { 'content-type': 'text/html; charset=utf-8' }, body, setCookies);
    },
  };
}

module.exports = { createResponses, canonicalName, PAGE_CSP };
