# API Gateway HTTP API。CloudFront（画面）のオリジン。
#
# カスタムドメインを画面と同じ名前にし、CloudFront から Host を転送する。アプリには Host がそのまま届き、
# Host・CSRF・MCP の Host の検査が Docker と同じに通る（PR #75「Host の流れ」）。

resource "aws_apigatewayv2_api" "app" {
  name          = var.name
  protocol_type = "HTTP"
  # 既定の execute-api のエンドポイントは使わない
  disable_execute_api_endpoint = true
}

resource "aws_apigatewayv2_domain_name" "app" {
  domain_name = var.app_domain

  domain_name_configuration {
    certificate_arn = aws_acm_certificate_validation.api.certificate_arn
    endpoint_type   = "REGIONAL"
    security_policy = "TLS_1_2"
  }
}

resource "aws_apigatewayv2_stage" "default" {
  api_id      = aws_apigatewayv2_api.app.id
  name        = "$default"
  auto_deploy = true

  default_route_settings {
    throttling_rate_limit  = var.api_throttling_rate_limit
    throttling_burst_limit = var.api_throttling_burst_limit
  }
}

resource "aws_apigatewayv2_api_mapping" "app" {
  api_id      = aws_apigatewayv2_api.app.id
  domain_name = aws_apigatewayv2_domain_name.app.id
  stage       = aws_apigatewayv2_stage.default.id
}

resource "aws_apigatewayv2_integration" "app" {
  api_id                 = aws_apigatewayv2_api.app.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.app.invoke_arn
  payload_format_version = "2.0"
  timeout_milliseconds   = 30000
}

resource "aws_lambda_permission" "api_invoke_app" {
  statement_id  = "AllowApiGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.app.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.app.execution_arn}/*/*"
}

# ---------------------------------------------------------------------------
# $default（画面・/api/upload・保護されたリソースのメタデータ）: CloudFront が付ける秘密のヘッダーを確かめる。
# CloudFront を通らずにカスタムドメインの向き先へ直接来たリクエストを断り、Lambda@Edge のログインを迂回させない

resource "random_password" "origin_verify" {
  length  = 48
  special = false
}

data "archive_file" "origin_verify" {
  type             = "zip"
  source_file      = "${path.module}/authorizer/src/index.js"
  output_path      = "${path.module}/.build/origin-verify.zip"
  output_file_mode = "0644"
}

resource "aws_cloudwatch_log_group" "origin_verify" {
  name              = "/aws/lambda/${var.name}-origin-verify"
  retention_in_days = var.log_retention_days
}

resource "aws_iam_role" "origin_verify" {
  name               = "${var.name}-origin-verify"
  assume_role_policy = data.aws_iam_policy_document.lambda_assume.json
}

resource "aws_iam_role_policy" "origin_verify" {
  name = "logs"
  role = aws_iam_role.origin_verify.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["logs:CreateLogStream", "logs:PutLogEvents"]
      Resource = "${aws_cloudwatch_log_group.origin_verify.arn}:*"
    }]
  })
}

resource "aws_lambda_function" "origin_verify" {
  function_name    = "${var.name}-origin-verify"
  role             = aws_iam_role.origin_verify.arn
  filename         = data.archive_file.origin_verify.output_path
  source_code_hash = data.archive_file.origin_verify.output_base64sha256
  handler          = "index.handler"
  runtime          = "nodejs24.x"
  architectures    = ["arm64"]
  memory_size      = 128
  timeout          = 3

  environment {
    variables = {
      ORIGIN_VERIFY_VALUES = join(",", concat([random_password.origin_verify.result], var.origin_verify_previous_values))
    }
  }

  logging_config {
    log_format = "Text"
    log_group  = aws_cloudwatch_log_group.origin_verify.name
  }

  depends_on = [aws_iam_role_policy.origin_verify]
}

resource "aws_apigatewayv2_authorizer" "origin_verify" {
  api_id                            = aws_apigatewayv2_api.app.id
  name                              = "origin-verify"
  authorizer_type                   = "REQUEST"
  authorizer_uri                    = aws_lambda_function.origin_verify.invoke_arn
  authorizer_payload_format_version = "2.0"
  enable_simple_responses           = true
  # ヘッダーが無ければ、オーソライザーを呼ばずに 401 にする
  identity_sources = ["$request.header.X-Origin-Verify"]
  # 結果はヘッダーの値ごとに 5 分キャッシュする
  authorizer_result_ttl_in_seconds = 300
}

resource "aws_lambda_permission" "api_invoke_origin_verify" {
  statement_id  = "AllowApiGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.origin_verify.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.app.execution_arn}/authorizers/${aws_apigatewayv2_authorizer.origin_verify.id}"
}

resource "aws_apigatewayv2_route" "default" {
  api_id             = aws_apigatewayv2_api.app.id
  route_key          = "$default"
  target             = "integrations/${aws_apigatewayv2_integration.app.id}"
  authorization_type = "CUSTOM"
  authorizer_id      = aws_apigatewayv2_authorizer.origin_verify.id
}

# ---------------------------------------------------------------------------
# ANY /mcp: Cognito のアクセストークンを確かめる（MCP の OAuth）。
# aud はクライアントが resource に指定した MCP の URL。秘密のヘッダーは見ないが、有効なトークンが要るので、
# CloudFront を通らずに届いても使えない

resource "aws_apigatewayv2_authorizer" "mcp" {
  api_id           = aws_apigatewayv2_api.app.id
  name             = "cognito"
  authorizer_type  = "JWT"
  identity_sources = ["$request.header.Authorization"]

  jwt_configuration {
    issuer   = local.cognito_issuer
    audience = [local.mcp_resource]
  }
}

resource "aws_apigatewayv2_route" "mcp" {
  api_id               = aws_apigatewayv2_api.app.id
  route_key            = "ANY /mcp"
  target               = "integrations/${aws_apigatewayv2_integration.app.id}"
  authorization_type   = "JWT"
  authorizer_id        = aws_apigatewayv2_authorizer.mcp.id
  authorization_scopes = [local.mcp_scope]
}
