# Lambda@Edge（us-east-1）。2 つの CloudFront に同じ関数を付け、Cognito のログインを確かめる。
#
# Lambda@Edge は環境変数を使えないので、設定は config.json に書いてパッケージに入れる。
# 公開クライアント（シークレットなし）なので、埋め込む秘密は無い。

locals {
  edge_function_name = "${var.name}-edge-auth"

  edge_config = {
    region               = var.region
    userPoolId           = aws_cognito_user_pool.this.id
    clientId             = aws_cognito_user_pool_client.browser.id
    cognitoDomain        = local.cognito_login_domain
    appHost              = var.app_domain
    docsHost             = local.docs_domain
    cookieDomain         = var.app_domain
    blobPrefix           = local.blob_prefix
    scopes               = join(" ", aws_cognito_user_pool_client.browser.allowed_oauth_scopes)
    sessionMaxAgeSeconds = local.session_days * 24 * 60 * 60
    # ID トークンの残りがこれより短ければ取り直す。有効期間を短くしたときに、毎回取り直さないよう 1/4 までにする
    refreshBeforeSeconds = floor(min(300, var.token_validity_minutes * 60 / 4))
    docsCsp              = local.docs_csp
  }
}

data "archive_file" "edge" {
  type             = "zip"
  output_path      = "${path.module}/.build/edge-auth.zip"
  output_file_mode = "0644"

  dynamic "source" {
    for_each = fileset("${path.module}/edge/src", "*.js")
    content {
      content  = file("${path.module}/edge/src/${source.value}")
      filename = source.value
    }
  }

  source {
    content  = jsonencode(local.edge_config)
    filename = "config.json"
  }
}

data "aws_iam_policy_document" "edge_assume" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["lambda.amazonaws.com", "edgelambda.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "edge" {
  name               = "${var.name}-edge-auth"
  assume_role_policy = data.aws_iam_policy_document.edge_assume.json
}

# ログは関数が動いたリージョンの /aws/lambda/us-east-1.<関数名> に出る
resource "aws_iam_role_policy" "edge" {
  name = "logs"
  role = aws_iam_role.edge.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = ["logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents"]
      Resource = [
        "arn:${local.partition}:logs:*:${local.account_id}:log-group:/aws/lambda/us-east-1.${local.edge_function_name}",
        "arn:${local.partition}:logs:*:${local.account_id}:log-group:/aws/lambda/us-east-1.${local.edge_function_name}:*",
      ]
    }]
  })
}

# 日本から使えばほぼ東京で動くので、東京のロググループだけは保存期間を決めて先に作る
resource "aws_cloudwatch_log_group" "edge" {
  name              = "/aws/lambda/us-east-1.${local.edge_function_name}"
  retention_in_days = var.log_retention_days
}

resource "aws_lambda_function" "edge" {
  provider = aws.us_east_1

  function_name    = local.edge_function_name
  role             = aws_iam_role.edge.arn
  filename         = data.archive_file.edge.output_path
  source_code_hash = data.archive_file.edge.output_base64sha256
  handler          = "index.handler"
  runtime          = "nodejs24.x"
  # Lambda@Edge は x86_64 だけ。トークンの更新で Cognito を呼ぶので、タイムアウトは数秒にする
  architectures = ["x86_64"]
  memory_size   = 128
  timeout       = 5
  # CloudFront に付けられるのは番号付きのバージョンだけ
  publish = true

  depends_on = [aws_iam_role_policy.edge]
}
