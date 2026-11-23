# AWS に触らずに、モックのプロバイダーで apply して、式と条件を確かめる（terraform test / tofu test）。
# 2 つのプロバイダー（既定と us-east-1）に同じモックを置く（mock_provider の source は OpenTofu との違いを避けて使わない）。

mock_provider "aws" {
  mock_data "aws_caller_identity" {
    defaults = { account_id = "123456789012" }
  }
  mock_data "aws_partition" {
    defaults = { partition = "aws" }
  }
  mock_data "aws_iam_policy_document" {
    defaults = { json = "{}" }
  }
  mock_data "aws_cloudfront_cache_policy" {
    defaults = { id = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad" }
  }
  mock_resource "aws_cognito_user_pool" {
    defaults = { id = "ap-northeast-1_MOCK", arn = "arn:aws:cognito-idp:ap-northeast-1:123456789012:userpool/ap-northeast-1_MOCK" }
  }
  mock_resource "aws_acm_certificate" {
    defaults = {
      arn = "arn:aws:acm:us-east-1:123456789012:certificate/mock"
      domain_validation_options = [
        { domain_name = "galley.example.com", resource_record_name = "_a.galley.example.com.", resource_record_type = "CNAME", resource_record_value = "_a.acm-validations.aws." },
        { domain_name = "docs.galley.example.com", resource_record_name = "_b.docs.galley.example.com.", resource_record_type = "CNAME", resource_record_value = "_b.acm-validations.aws." },
      ]
    }
  }
  mock_resource "aws_cloudfront_distribution" {
    defaults = { arn = "arn:aws:cloudfront::123456789012:distribution/EMOCK", domain_name = "d111111abcdef8.cloudfront.net", hosted_zone_id = "Z2FDTNDATAQYW2" }
  }
  mock_resource "aws_lambda_function" {
    defaults = {
      arn           = "arn:aws:lambda:us-east-1:123456789012:function:mock"
      qualified_arn = "arn:aws:lambda:us-east-1:123456789012:function:mock:1"
      invoke_arn    = "arn:aws:apigateway:ap-northeast-1:lambda:path/2015-03-31/functions/arn:aws:lambda:ap-northeast-1:123456789012:function:mock/invocations"
    }
  }
  mock_resource "aws_iam_role" {
    defaults = { arn = "arn:aws:iam::123456789012:role/mock" }
  }
  mock_resource "aws_s3_bucket" {
    defaults = { arn = "arn:aws:s3:::mock", bucket_regional_domain_name = "mock.s3.ap-northeast-1.amazonaws.com" }
  }
  mock_resource "aws_dynamodb_table" {
    defaults = { arn = "arn:aws:dynamodb:ap-northeast-1:123456789012:table/galley" }
  }
  mock_resource "aws_cloudwatch_log_group" {
    defaults = { arn = "arn:aws:logs:ap-northeast-1:123456789012:log-group:mock" }
  }
  mock_resource "aws_apigatewayv2_api" {
    defaults = { execution_arn = "arn:aws:execute-api:ap-northeast-1:123456789012:mock" }
  }
  mock_resource "aws_ecr_repository" {
    defaults = { repository_url = "123456789012.dkr.ecr.ap-northeast-1.amazonaws.com/galley" }
  }
}

mock_provider "aws" {
  alias = "us_east_1"

  mock_data "aws_caller_identity" {
    defaults = { account_id = "123456789012" }
  }
  mock_data "aws_partition" {
    defaults = { partition = "aws" }
  }
  mock_data "aws_iam_policy_document" {
    defaults = { json = "{}" }
  }
  mock_data "aws_cloudfront_cache_policy" {
    defaults = { id = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad" }
  }
  mock_resource "aws_cognito_user_pool" {
    defaults = { id = "ap-northeast-1_MOCK", arn = "arn:aws:cognito-idp:ap-northeast-1:123456789012:userpool/ap-northeast-1_MOCK" }
  }
  mock_resource "aws_acm_certificate" {
    defaults = {
      arn = "arn:aws:acm:us-east-1:123456789012:certificate/mock"
      domain_validation_options = [
        { domain_name = "galley.example.com", resource_record_name = "_a.galley.example.com.", resource_record_type = "CNAME", resource_record_value = "_a.acm-validations.aws." },
        { domain_name = "docs.galley.example.com", resource_record_name = "_b.docs.galley.example.com.", resource_record_type = "CNAME", resource_record_value = "_b.acm-validations.aws." },
      ]
    }
  }
  mock_resource "aws_cloudfront_distribution" {
    defaults = { arn = "arn:aws:cloudfront::123456789012:distribution/EMOCK", domain_name = "d111111abcdef8.cloudfront.net", hosted_zone_id = "Z2FDTNDATAQYW2" }
  }
  mock_resource "aws_lambda_function" {
    defaults = {
      arn           = "arn:aws:lambda:us-east-1:123456789012:function:mock"
      qualified_arn = "arn:aws:lambda:us-east-1:123456789012:function:mock:1"
      invoke_arn    = "arn:aws:apigateway:ap-northeast-1:lambda:path/2015-03-31/functions/arn:aws:lambda:ap-northeast-1:123456789012:function:mock/invocations"
    }
  }
  mock_resource "aws_iam_role" {
    defaults = { arn = "arn:aws:iam::123456789012:role/mock" }
  }
  mock_resource "aws_s3_bucket" {
    defaults = { arn = "arn:aws:s3:::mock", bucket_regional_domain_name = "mock.s3.ap-northeast-1.amazonaws.com" }
  }
  mock_resource "aws_dynamodb_table" {
    defaults = { arn = "arn:aws:dynamodb:ap-northeast-1:123456789012:table/galley" }
  }
  mock_resource "aws_cloudwatch_log_group" {
    defaults = { arn = "arn:aws:logs:ap-northeast-1:123456789012:log-group:mock" }
  }
  mock_resource "aws_apigatewayv2_api" {
    defaults = { execution_arn = "arn:aws:execute-api:ap-northeast-1:123456789012:mock" }
  }
  mock_resource "aws_ecr_repository" {
    defaults = { repository_url = "123456789012.dkr.ecr.ap-northeast-1.amazonaws.com/galley" }
  }
}

variables {
  app_domain            = "galley.example.com"
  cognito_domain_prefix = "acme-galley"
  image_tag             = "v0.1.0"
}

run "route53_zone_given" {
  variables {
    route53_zone_id = "Z0000000000000000000"
  }

  assert {
    condition     = length(aws_route53_record.acm_validation) == 2 && length(aws_route53_record.app) == 2 && length(aws_route53_record.docs) == 2
    error_message = "ゾーンを渡したら、検証レコードと別名レコード（A・AAAA）を作る"
  }

  assert {
    condition     = length(output.dns_records) == 0 && length(output.acm_validation_records) == 0
    error_message = "ゾーンを渡したら、手で作るレコードは出さない"
  }
}

run "route53_zone_not_given" {
  assert {
    condition     = length(aws_route53_record.acm_validation) == 0 && length(aws_route53_record.app) == 0 && length(aws_route53_record.docs) == 0
    error_message = "ゾーンを渡さなければ、Route 53 のレコードは作らない"
  }

  assert {
    condition     = [for r in output.dns_records : r.name] == ["galley.example.com", "docs.galley.example.com"]
    error_message = "手で作るレコードに、2 つのホスト名を出す"
  }

  assert {
    condition     = length(output.acm_validation_records) == 2
    error_message = "同じホスト名の検証レコードは 1 つにまとめる（画面と資料配信の 2 つ）"
  }
}

run "app_lambda" {
  assert {
    condition = aws_lambda_function.app.environment[0].variables == tomap({
      GALLEY_DATA_DIR                 = "/tmp"
      ALLOWED_HOSTS                   = "galley.example.com"
      GALLEY_APP_URL                  = "https://galley.example.com"
      GALLEY_VIEWER_URL               = "https://docs.galley.example.com"
      GALLEY_VIEWER_ADDR              = "127.0.0.1:8081"
      GALLEY_MAX_HTML_SIZE            = "4MB"
      GALLEY_BLOB_STORE               = "s3://galley-blobs-123456789012/blobs"
      GALLEY_DB                       = "dynamodb://galley"
      MCP_OAUTH_RESOURCE              = "https://galley.example.com/mcp"
      MCP_OAUTH_AUTHORIZATION_SERVERS = "https://cognito-idp.ap-northeast-1.amazonaws.com/ap-northeast-1_MOCK"
      MCP_OAUTH_SCOPES                = "openid,https://galley.example.com/mcp/access"
      TZ                              = "Asia/Tokyo"
    })
    error_message = "アプリの環境変数が README の表と合わない"
  }

  assert {
    condition     = tolist(aws_lambda_function.app.architectures) == tolist(["arm64"]) && aws_lambda_function.app.memory_size == 1024 && aws_lambda_function.app.timeout == 30
    error_message = "Lambda は arm64・1 GB・30 秒"
  }

  assert {
    condition     = endswith(aws_lambda_function.app.image_uri, ":v0.1.0")
    error_message = "イメージは image_tag のタグを使う"
  }
}

run "mcp_route_and_behaviors" {
  assert {
    condition     = toset(aws_apigatewayv2_authorizer.mcp.jwt_configuration[0].audience) == toset(["https://galley.example.com/mcp"])
    error_message = "JWT オーソライザーの aud は MCP の URL"
  }

  assert {
    condition     = toset(aws_apigatewayv2_route.mcp.authorization_scopes) == toset(["https://galley.example.com/mcp/access"])
    error_message = "/mcp には MCP のスコープを求める"
  }

  assert {
    condition     = aws_apigatewayv2_api.app.disable_execute_api_endpoint
    error_message = "既定の execute-api のエンドポイントは無効にする"
  }

  assert {
    condition     = [for b in aws_cloudfront_distribution.app.ordered_cache_behavior : b.path_pattern] == ["/mcp", "/.well-known/oauth-protected-resource", "/.well-known/oauth-protected-resource/mcp"]
    error_message = "Lambda@Edge を付けないパスは、MCP とメタデータの完全一致だけ"
  }

  assert {
    condition     = alltrue([for b in aws_cloudfront_distribution.app.ordered_cache_behavior : length(b.lambda_function_association) == 0])
    error_message = "MCP のパスには Lambda@Edge を付けない"
  }

  assert {
    condition     = length(aws_cloudfront_distribution.app.default_cache_behavior[0].lambda_function_association) == 1
    error_message = "画面には Lambda@Edge（viewer-request）を付ける"
  }

  assert {
    condition     = toset([for a in aws_cloudfront_distribution.docs.default_cache_behavior[0].lambda_function_association : a.event_type]) == toset(["viewer-request", "origin-response"])
    error_message = "資料配信には viewer-request と origin-response の Lambda@Edge を付ける"
  }

  assert {
    condition     = aws_cloudfront_origin_request_policy.app.cookies_config[0].cookies[0].items == toset(["galley_author"])
    error_message = "アプリに渡す Cookie は galley_author だけ"
  }
}

run "edge_config" {
  assert {
    condition     = local.edge_config.docsCsp == local.docs_csp
    error_message = "Lambda@Edge と応答ヘッダーポリシーの CSP は同じ"
  }

  assert {
    condition     = local.edge_config.cookieDomain == "galley.example.com" && local.edge_config.docsHost == "docs.galley.example.com"
    error_message = "Cookie は画面のホスト名（資料配信の親）に出す"
  }

  assert {
    condition     = local.edge_config.sessionMaxAgeSeconds == 2592000
    error_message = "Cookie の期限はリフレッシュトークンと同じ 30 日"
  }

  assert {
    condition     = aws_lambda_function.edge.publish && aws_lambda_function.edge.runtime == "nodejs24.x"
    error_message = "Lambda@Edge は番号付きのバージョンを出す"
  }
}

run "mfa_on" {
  variables {
    mfa_configuration = "ON"
  }

  assert {
    condition     = aws_cognito_user_pool.this.mfa_configuration == "ON" && aws_cognito_user_pool.this.software_token_mfa_configuration[0].enabled
    error_message = "MFA を ON にしたら、認証アプリ（TOTP）を使う"
  }
}

run "deletion_protection_off" {
  variables {
    deletion_protection = false
  }

  assert {
    condition     = aws_s3_bucket.blobs.force_destroy && !aws_dynamodb_table.this.deletion_protection_enabled && aws_cognito_user_pool.this.deletion_protection == "INACTIVE"
    error_message = "削除保護を外したら、destroy で消せるようにする"
  }
}

run "image_tag_required" {
  command = plan

  variables {
    image_tag = null
  }

  expect_failures = [aws_lambda_function.app]
}
