'use strict';

/**
 * Cognito（JWKS・トークンのエンドポイント）に届かない、または想定しない応答が返ったとき。
 * ログインの画面に回しても同じ理由で失敗するので、呼び出し側は 503 を返す。
 */
class ServiceUnavailableError extends Error {
  constructor(message) {
    super(message);
    this.name = 'ServiceUnavailableError';
  }
}

module.exports = { ServiceUnavailableError };
