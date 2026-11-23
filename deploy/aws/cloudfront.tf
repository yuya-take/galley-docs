# CloudFront を 2 つ置く。
#
# - app_domain: 画面と MCP。オリジンは API Gateway のカスタムドメイン。Host を転送する
# - docs.<app_domain>: 資料配信。オリジンは S3（OAC）。Lambda を通さない
#
# どちらにも Lambda@Edge（viewer-request）を付けてログインを確かめる。ただし MCP クライアントは Cookie を
# 持たないので、/mcp と保護されたリソースのメタデータには付けない（MCP は API Gateway の JWT オーソライザーで守る）。
# 標準ログは出さない（出すなら Cookie を記録しない設定にする）。

data "aws_cloudfront_cache_policy" "caching_disabled" {
  name = "Managed-CachingDisabled"
}

# ---------------------------------------------------------------------------
# 画面と MCP

# アプリに渡すもの。Host を含むすべてのヘッダー（Authorization も含む）と問い合わせ文字列。
# Cookie は更新者名（galley_author）だけにし、ログインのトークンはアプリに渡さない
resource "aws_cloudfront_origin_request_policy" "app" {
  name    = "${var.name}-app"
  comment = "Galley: Host を含むヘッダーを転送し、Cookie は galley_author だけ"

  headers_config {
    header_behavior = "allViewer"
  }

  cookies_config {
    cookie_behavior = "whitelist"
    cookies {
      items = ["galley_author"]
    }
  }

  query_strings_config {
    query_string_behavior = "all"
  }
}

resource "aws_cloudfront_origin_request_policy" "mcp" {
  name    = "${var.name}-mcp"
  comment = "Galley: MCP。Host と Authorization を含むヘッダーを転送し、Cookie は渡さない"

  headers_config {
    header_behavior = "allViewer"
  }

  cookies_config {
    cookie_behavior = "none"
  }

  query_strings_config {
    query_string_behavior = "all"
  }
}

resource "aws_cloudfront_response_headers_policy" "app" {
  name    = "${var.name}-app"
  comment = "Galley: 画面"

  security_headers_config {
    strict_transport_security {
      access_control_max_age_sec = 63072000
      include_subdomains         = true
      override                   = true
    }
  }
}

locals {
  app_origin_id = "api-gateway"

  # /mcp は完全一致にする。/mcp* にすると /mcpx（プロジェクトの URL）までログインなしで届いてしまう
  mcp_paths = ["/mcp", "/.well-known/oauth-protected-resource", "/.well-known/oauth-protected-resource/mcp"]
}

resource "aws_cloudfront_distribution" "app" {
  enabled         = true
  comment         = "${var.name}: 画面と MCP"
  aliases         = [var.app_domain]
  price_class     = var.price_class
  http_version    = "http2and3"
  is_ipv6_enabled = true

  origin {
    origin_id   = local.app_origin_id
    domain_name = aws_apigatewayv2_domain_name.app.domain_name_configuration[0].target_domain_name

    custom_origin_config {
      http_port              = 80
      https_port             = 443
      origin_protocol_policy = "https-only"
      origin_ssl_protocols   = ["TLSv1.2"]
      # API Gateway の統合の上限（30 秒）に合わせる
      origin_read_timeout = 30
    }

    # CloudFront を通ったことを API Gateway の Lambda オーソライザーに示す
    custom_header {
      name  = "X-Origin-Verify"
      value = random_password.origin_verify.result
    }
  }

  default_cache_behavior {
    target_origin_id           = local.app_origin_id
    viewer_protocol_policy     = "redirect-to-https"
    allowed_methods            = ["GET", "HEAD", "OPTIONS", "PUT", "POST", "PATCH", "DELETE"]
    cached_methods             = ["GET", "HEAD"]
    compress                   = true
    cache_policy_id            = data.aws_cloudfront_cache_policy.caching_disabled.id
    origin_request_policy_id   = aws_cloudfront_origin_request_policy.app.id
    response_headers_policy_id = aws_cloudfront_response_headers_policy.app.id

    lambda_function_association {
      event_type   = "viewer-request"
      lambda_arn   = aws_lambda_function.edge.qualified_arn
      include_body = false
    }
  }

  dynamic "ordered_cache_behavior" {
    for_each = local.mcp_paths
    content {
      path_pattern               = ordered_cache_behavior.value
      target_origin_id           = local.app_origin_id
      viewer_protocol_policy     = "https-only"
      allowed_methods            = ["GET", "HEAD", "OPTIONS", "PUT", "POST", "PATCH", "DELETE"]
      cached_methods             = ["GET", "HEAD"]
      compress                   = true
      cache_policy_id            = data.aws_cloudfront_cache_policy.caching_disabled.id
      origin_request_policy_id   = aws_cloudfront_origin_request_policy.mcp.id
      response_headers_policy_id = aws_cloudfront_response_headers_policy.app.id
    }
  }

  restrictions {
    geo_restriction {
      restriction_type = "none"
    }
  }

  viewer_certificate {
    acm_certificate_arn      = aws_acm_certificate_validation.cloudfront.certificate_arn
    ssl_support_method       = "sni-only"
    minimum_protocol_version = "TLSv1.2_2021"
  }
}

# ---------------------------------------------------------------------------
# 資料配信

resource "aws_cloudfront_origin_access_control" "docs" {
  name                              = "${var.name}-docs"
  description                       = "Galley: 資料配信から S3 を読む"
  origin_access_control_origin_type = "s3"
  signing_behavior                  = "always"
  signing_protocol                  = "sigv4"
}

# 中身で決まる URL なので長くキャッシュする。キャッシュのキーは、書き換えたあとのパス（S3 のキー）と download だけ
resource "aws_cloudfront_cache_policy" "docs" {
  name        = "${var.name}-docs"
  comment     = "Galley: 資料配信"
  min_ttl     = 0
  default_ttl = 31536000
  max_ttl     = 31536000

  parameters_in_cache_key_and_forwarded_to_origin {
    enable_accept_encoding_gzip   = true
    enable_accept_encoding_brotli = true

    cookies_config {
      cookie_behavior = "none"
    }

    headers_config {
      header_behavior = "none"
    }

    query_strings_config {
      query_string_behavior = "whitelist"
      query_strings {
        items = ["download"]
      }
    }
  }
}

# 資料配信のすべての応答に付ける（docs/design.md「ブラウザ側の制限」）。関数が返す応答には関数でも付けている
resource "aws_cloudfront_response_headers_policy" "docs" {
  name    = "${var.name}-docs"
  comment = "Galley: 資料配信の CSP（sandbox）・nosniff・no-referrer"

  security_headers_config {
    content_security_policy {
      content_security_policy = local.docs_csp
      override                = true
    }

    content_type_options {
      override = true
    }

    referrer_policy {
      referrer_policy = "no-referrer"
      override        = true
    }

    strict_transport_security {
      access_control_max_age_sec = 63072000
      include_subdomains         = true
      override                   = true
    }
  }
}

locals {
  docs_origin_id = "s3-blobs"
}

resource "aws_cloudfront_distribution" "docs" {
  enabled         = true
  comment         = "${var.name}: 資料配信"
  aliases         = [local.docs_domain]
  price_class     = var.price_class
  http_version    = "http2and3"
  is_ipv6_enabled = true

  origin {
    origin_id                = local.docs_origin_id
    domain_name              = aws_s3_bucket.blobs.bucket_regional_domain_name
    origin_access_control_id = aws_cloudfront_origin_access_control.docs.id
  }

  default_cache_behavior {
    target_origin_id           = local.docs_origin_id
    viewer_protocol_policy     = "redirect-to-https"
    allowed_methods            = ["GET", "HEAD"]
    cached_methods             = ["GET", "HEAD"]
    compress                   = true
    cache_policy_id            = aws_cloudfront_cache_policy.docs.id
    response_headers_policy_id = aws_cloudfront_response_headers_policy.docs.id

    # ログインを確かめ、/b/<hash> を S3 のキーに書き換える
    lambda_function_association {
      event_type   = "viewer-request"
      lambda_arn   = aws_lambda_function.edge.qualified_arn
      include_body = false
    }

    # キャッシュの指定と、ダウンロードの Content-Disposition を付ける（キャッシュに入る前に 1 回だけ動く）
    lambda_function_association {
      event_type   = "origin-response"
      lambda_arn   = aws_lambda_function.edge.qualified_arn
      include_body = false
    }
  }

  restrictions {
    geo_restriction {
      restriction_type = "none"
    }
  }

  viewer_certificate {
    acm_certificate_arn      = aws_acm_certificate_validation.cloudfront.certificate_arn
    ssl_support_method       = "sni-only"
    minimum_protocol_version = "TLSv1.2_2021"
  }
}
