'use strict';

const { canonicalName } = require('./responses');

/** 資料配信の URL。ハッシュは 64 文字の小文字の16進数だけ（docs/design.md「資料の配信 URL」）。 */
const BLOB_PATH = /^\/b\/([0-9a-f]{64})$/;
/** 資料の slug の規則（crates/core の Slug と DocumentSlug）。 */
const SLUG_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const SLUG_MAX_LEN = 64;
const RESERVED_DOCUMENT_SLUGS = new Set(['settings']);
const CACHE_IMMUTABLE = 'public, max-age=31536000, immutable';

function isDocumentSlug(value) {
  return (
    typeof value === 'string' &&
    value.length <= SLUG_MAX_LEN &&
    SLUG_PATTERN.test(value) &&
    !RESERVED_DOCUMENT_SLUGS.has(value)
  );
}

/** S3 のキー。アプリの `BlobHash::storage_key` と同じ `<prefix>/ab/cd/<hash>.html`。 */
function blobKey(prefix, hash) {
  const key = `${hash.slice(0, 2)}/${hash.slice(2, 4)}/${hash}.html`;
  return prefix ? `${prefix}/${key}` : key;
}

/**
 * `/b/<hash>?download=<slug>` を S3 のキーに書き換える。
 *
 * 戻り値は `{ uri, querystring }`、またはアプリの資料配信と同じ断り方（`{ status: 404 }` か `{ status: 400 }`）。
 * 問い合わせ文字列は `download` だけを残す（キャッシュのキーも `download` だけ）。
 */
function rewriteBlobRequest(uri, querystring, prefix) {
  const match = BLOB_PATH.exec(uri);
  if (!match) {
    return { status: 404 };
  }
  const downloads = new URLSearchParams(querystring).getAll('download');
  if (downloads.length > 1 || (downloads.length === 1 && !isDocumentSlug(downloads[0]))) {
    return { status: 400 };
  }
  return {
    uri: `/${blobKey(prefix, match[1])}`,
    querystring: downloads.length === 1 ? `download=${downloads[0]}` : '',
  };
}

function setHeader(response, name, value) {
  response.headers[name] = [{ key: canonicalName(name), value }];
}

/**
 * S3 の応答を資料配信の応答に整える（origin-response）。キャッシュに入る前に 1 回だけ動く。
 *
 * - 200: 中身で決まる URL なので長くキャッシュさせる。`download` があれば `Content-Disposition` を付ける
 * - 404 など: S3 の XML のエラーを、アプリの資料配信と同じ素のテキストに置き換え、キャッシュさせない
 */
function finishBlobResponse(request, response, securityHeaders) {
  const status = Number(response.status);
  if (status === 200) {
    setHeader(response, 'cache-control', CACHE_IMMUTABLE);
    const download = new URLSearchParams(request.querystring).get('download');
    if (isDocumentSlug(download)) {
      setHeader(response, 'content-disposition', `attachment; filename="${download}.html"`);
    }
    return response;
  }
  if (status < 400) {
    return response;
  }
  // Via と Transfer-Encoding は読み取り専用なので、ヘッダーは作り直さずに書き換える
  for (const name of ['content-length', 'content-disposition', 'etag', 'last-modified']) {
    delete response.headers[name];
  }
  for (const [name, value] of Object.entries(securityHeaders)) {
    setHeader(response, name, value);
  }
  setHeader(response, 'content-type', 'text/plain; charset=utf-8');
  response.body = status === 404 ? '資料が見つかりません\n' : '資料を表示できません\n';
  response.bodyEncoding = 'text';
  return response;
}

module.exports = { rewriteBlobRequest, finishBlobResponse, isDocumentSlug, blobKey, CACHE_IMMUTABLE };
