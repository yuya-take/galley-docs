# アプリ（画面と MCP）の Lambda。Docker と同じイメージを Lambda Web Adapter で動かす（README の「Lambda で動かす」）。

# Lambda のコンテナイメージは ECR からしか取れないので、ghcr.io のイメージをここに複製する（README の手順）
resource "aws_ecr_repository" "app" {
  name                 = var.name
  image_tag_mutability = "IMMUTABLE"
  # ghcr.io から複製し直せるので、destroy のときは中のイメージごと消す
  force_delete = true
}

resource "aws_ecr_lifecycle_policy" "app" {
  repository = aws_ecr_repository.app.name
  policy = jsonencode({
    rules = [{
      rulePriority = 1
      description  = "新しいほうから 10 個だけ残す"
      selection = {
        tagStatus   = "any"
        countType   = "imageCountMoreThan"
        countNumber = 10
      }
      action = { type = "expire" }
    }]
  })
}

# Lambda がイメージを取れるようにする
resource "aws_ecr_repository_policy" "app" {
  repository = aws_ecr_repository.app.name
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "LambdaPull"
      Effect    = "Allow"
      Principal = { Service = "lambda.amazonaws.com" }
      Action    = ["ecr:BatchGetImage", "ecr:GetDownloadUrlForLayer"]
      Condition = {
        StringLike = {
          "aws:sourceArn" = "arn:${local.partition}:lambda:${var.region}:${local.account_id}:function:*"
        }
      }
    }]
  })
}

resource "aws_cloudwatch_log_group" "app" {
  name              = "/aws/lambda/${var.name}"
  retention_in_days = var.log_retention_days
}

data "aws_iam_policy_document" "lambda_assume" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["lambda.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "app" {
  name               = "${var.name}-app"
  assume_role_policy = data.aws_iam_policy_document.lambda_assume.json
}

data "aws_iam_policy_document" "app" {
  statement {
    sid       = "Logs"
    actions   = ["logs:CreateLogStream", "logs:PutLogEvents"]
    resources = ["${aws_cloudwatch_log_group.app.arn}:*"]
  }

  # #76 の DynamoDB の adapter が使う操作
  statement {
    sid = "DynamoDB"
    actions = [
      "dynamodb:GetItem",
      "dynamodb:PutItem",
      "dynamodb:UpdateItem",
      "dynamodb:DeleteItem",
      "dynamodb:Query",
      "dynamodb:TransactWriteItems",
      "dynamodb:ConditionCheckItem",
    ]
    resources = [aws_dynamodb_table.this.arn]
  }

  statement {
    sid       = "BlobObjects"
    actions   = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"]
    resources = ["${aws_s3_bucket.blobs.arn}/${local.blob_prefix}/*"]
  }

  # アプリは書く前に HEAD で有無を見る。ListBucket が無いと、無いオブジェクトが 404 ではなく 403 になり、登録が失敗する
  statement {
    sid       = "BlobList"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.blobs.arn]
  }
}

resource "aws_iam_role_policy" "app" {
  name   = "app"
  role   = aws_iam_role.app.id
  policy = data.aws_iam_policy_document.app.json
}

resource "aws_lambda_function" "app" {
  function_name = var.name
  role          = aws_iam_role.app.arn
  package_type  = "Image"
  image_uri     = "${aws_ecr_repository.app.repository_url}:${coalesce(var.image_tag, "unset")}"
  architectures = ["arm64"]
  memory_size   = 1024
  # API Gateway HTTP API の統合の上限（30 秒）に合わせる
  timeout = 30

  environment {
    variables = merge({
      GALLEY_DATA_DIR      = "/tmp"
      ALLOWED_HOSTS        = var.app_domain
      GALLEY_APP_URL       = local.app_origin
      GALLEY_VIEWER_URL    = local.docs_origin
      GALLEY_VIEWER_ADDR   = "127.0.0.1:8081"
      GALLEY_MAX_HTML_SIZE = var.max_html_size
      GALLEY_BLOB_STORE    = "s3://${aws_s3_bucket.blobs.bucket}/${local.blob_prefix}"
      GALLEY_DB            = "dynamodb://${aws_dynamodb_table.this.name}"
      # MCP の保護されたリソースのメタデータ（#78）。トークンは API Gateway の JWT オーソライザーが確かめるので、
      # MCP_TOKEN は設定しない
      MCP_OAUTH_RESOURCE              = local.mcp_resource
      MCP_OAUTH_AUTHORIZATION_SERVERS = local.cognito_issuer
      MCP_OAUTH_SCOPES                = "openid,${local.mcp_scope}"
      TZ                              = var.timezone
    }, var.app_environment)
  }

  logging_config {
    log_format = "Text"
    log_group  = aws_cloudwatch_log_group.app.name
  }

  depends_on = [aws_iam_role_policy.app, aws_ecr_repository_policy.app]

  lifecycle {
    precondition {
      condition     = var.image_tag != null
      error_message = "image_tag を設定してください。先に ECR にイメージを複製します（deploy/aws/README.md の「Terraform で立ち上げる」）。"
    }
  }
}
