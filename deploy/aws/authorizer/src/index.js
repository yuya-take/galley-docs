'use strict';

const crypto = require('node:crypto');

function digest(value) {
  return crypto.createHash('sha256').update(value).digest();
}

/**
 * API Gateway HTTP API の Lambda オーソライザー（ペイロード 2.0、シンプルな応答）。
 *
 * CloudFront がオリジンのカスタムヘッダーで付ける `X-Origin-Verify` を確かめ、CloudFront を通らずに
 * API Gateway のカスタムドメインの向き先へ直接来たリクエストを断る（Lambda@Edge のログインを迂回させない）。
 * 値は複数を受け付ける（入れ替えの間、古い値と新しい値の両方を通すため）。
 */
function createHandler(values) {
  const expected = values.filter((value) => value.length > 0).map(digest);
  return async function handler(event) {
    const received = (event.headers && event.headers['x-origin-verify']) || '';
    // 長さで早く返さないよう、ハッシュをそろえて全部と比べる
    const actual = digest(received);
    let matched = false;
    for (const value of expected) {
      matched = crypto.timingSafeEqual(actual, value) || matched;
    }
    return { isAuthorized: received.length > 0 && matched };
  };
}

function valuesFromEnv(env) {
  return (env.ORIGIN_VERIFY_VALUES || '')
    .split(',')
    .map((value) => value.trim())
    .filter((value) => value.length > 0);
}

exports.handler = createHandler(valuesFromEnv(process.env));
exports.createHandler = createHandler;
exports.valuesFromEnv = valuesFromEnv;
