'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');

const { rewriteBlobRequest, isDocumentSlug, blobKey } = require('../src/docs');
const { parseCookies } = require('../src/cookies');

const HASH = 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855';

test('S3 のキーはアプリの BlobHash::storage_key と同じ形', () => {
  assert.equal(blobKey('blobs', HASH), `blobs/e3/b0/${HASH}.html`);
  assert.equal(blobKey('', HASH), `e3/b0/${HASH}.html`);
});

test('download はアプリの資料配信と同じ規則で受け付ける', () => {
  // crates/server/src/viewer.rs の download_rejects_names_other_than_slugs と同じ値
  for (const query of [
    'download=',
    'download',
    'download=Keikaku',
    'download=keikaku.html',
    'download=a%22b',
    'download=a%0D%0ASet-Cookie:x',
    'download=..%2Fkeikaku',
    'download=settings',
    'download=keikaku&download=other',
    'download=-a',
    'download=a-',
    'download=a--b',
    `download=${'a'.repeat(65)}`,
  ]) {
    assert.deepEqual(rewriteBlobRequest(`/b/${HASH}`, query, 'blobs'), { status: 400 }, query);
  }
  for (const slug of ['keikaku', 'doc-3f9a1c2e', 'a', '2026', 'a'.repeat(64)]) {
    assert.ok(isDocumentSlug(slug), slug);
    assert.deepEqual(rewriteBlobRequest(`/b/${HASH}`, `download=${slug}`, 'blobs'), {
      uri: `/blobs/e3/b0/${HASH}.html`,
      querystring: `download=${slug}`,
    });
  }
});

test('ハッシュは 64 文字の小文字の16進数だけ', () => {
  for (const uri of [`/b/${HASH.slice(1)}`, `/b/${HASH}0`, `/b/${HASH.toUpperCase()}`, `/b/${HASH}.html`, '/b/', '/']) {
    assert.deepEqual(rewriteBlobRequest(uri, '', 'blobs'), { status: 404 }, uri);
  }
});

test('Cookie ヘッダーを読む', () => {
  const cookies = parseCookies({
    cookie: [{ key: 'Cookie', value: 'a=1; b=x=y;  c=; =d; e' }, { key: 'Cookie', value: 'a=2; f=3' }],
  });
  assert.equal(cookies.get('a'), '1');
  assert.equal(cookies.get('b'), 'x=y');
  assert.equal(cookies.get('c'), '');
  assert.equal(cookies.get('f'), '3');
  assert.equal(cookies.has('e'), false);
  assert.equal(parseCookies({}).size, 0);
});
