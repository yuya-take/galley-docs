locals {
  account_id  = data.aws_caller_identity.current.account_id
  partition   = data.aws_partition.current.partition
  docs_domain = "docs.${var.app_domain}"
  app_origin  = "https://${var.app_domain}"
  docs_origin = "https://${local.docs_domain}"

  # Cognito。発行元は ORIGINAL の形（プロバイダーが IssuerConfiguration を設定できないため、既定のまま）
  cognito_issuer       = "https://cognito-idp.${var.region}.amazonaws.com/${aws_cognito_user_pool.this.id}"
  cognito_login_domain = "${aws_cognito_user_pool_domain.this.domain}.auth.${var.region}.amazoncognito.com"

  # MCP の OAuth。クライアントが resource に指定する URL がアクセストークンの aud になる
  mcp_resource = "${local.app_origin}/mcp"
  mcp_scope    = "${local.mcp_resource}/access"

  # ログインのセッション（リフレッシュトークンと Cookie）の長さ
  session_days = 30

  blob_prefix = trim(var.blob_prefix, "/")
  bucket_name = coalesce(var.bucket_name, "${var.name}-blobs-${local.account_id}")

  # 資料配信のすべての応答に付ける CSP（docs/design.md「ブラウザ側の制限」）
  docs_csp = "sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; form-action 'none'; base-uri 'none'"

  tags = merge({ Application = "galley" }, var.tags)
}
