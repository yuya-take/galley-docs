# 資料の実体（S3）と DB（DynamoDB）。

resource "aws_s3_bucket" "blobs" {
  bucket = local.bucket_name
  # 削除保護を外したときだけ、中身ごと消せるようにする
  force_destroy = !var.deletion_protection
}

resource "aws_s3_bucket_public_access_block" "blobs" {
  bucket = aws_s3_bucket.blobs.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "blobs" {
  bucket = aws_s3_bucket.blobs.id

  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "blobs" {
  bucket = aws_s3_bucket.blobs.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_versioning" "blobs" {
  bucket = aws_s3_bucket.blobs.id

  versioning_configuration {
    status = "Enabled"
  }
}

# 上書き・削除された実体（古い版）は、noncurrent_version_retention_days 日で消す。
# 物理削除（galley purge）した資料も、この日数のあいだは古い版として残る（README の「物理削除のあと」）
resource "aws_s3_bucket_lifecycle_configuration" "blobs" {
  bucket = aws_s3_bucket.blobs.id

  rule {
    id     = "expire-noncurrent-versions"
    status = "Enabled"

    filter {}

    noncurrent_version_expiration {
      noncurrent_days = var.noncurrent_version_retention_days
    }

    expiration {
      expired_object_delete_marker = true
    }

    abort_incomplete_multipart_upload {
      days_after_initiation = 1
    }
  }

  depends_on = [aws_s3_bucket_versioning.blobs]
}

data "aws_iam_policy_document" "blobs" {
  # 読めるのは資料配信の CloudFront だけ（OAC）
  statement {
    sid       = "AllowDocsCloudFrontRead"
    actions   = ["s3:GetObject"]
    resources = ["${aws_s3_bucket.blobs.arn}/${local.blob_prefix}/*"]

    principals {
      type        = "Service"
      identifiers = ["cloudfront.amazonaws.com"]
    }

    condition {
      test     = "StringEquals"
      variable = "AWS:SourceArn"
      values   = [aws_cloudfront_distribution.docs.arn]
    }
  }

  # 無い実体に 403 ではなく 404 を返させる
  statement {
    sid       = "AllowDocsCloudFrontList"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.blobs.arn]

    principals {
      type        = "Service"
      identifiers = ["cloudfront.amazonaws.com"]
    }

    condition {
      test     = "StringEquals"
      variable = "AWS:SourceArn"
      values   = [aws_cloudfront_distribution.docs.arn]
    }
  }

  # 書けるのはアプリの Lambda のロールだけ。CloudFront は読み出しのたびに中身のハッシュを確かめないので、
  # アップロードの検査を通ったものだけがここに入るようにする（PR #75「資料配信を S3 から配る」）
  statement {
    sid       = "DenyPutExceptApp"
    effect    = "Deny"
    actions   = ["s3:PutObject"]
    resources = ["${aws_s3_bucket.blobs.arn}/*"]

    principals {
      type        = "*"
      identifiers = ["*"]
    }

    condition {
      test     = "ArnNotEquals"
      variable = "aws:PrincipalArn"
      values   = [aws_iam_role.app.arn]
    }
  }

  statement {
    sid       = "DenyInsecureTransport"
    effect    = "Deny"
    actions   = ["s3:*"]
    resources = [aws_s3_bucket.blobs.arn, "${aws_s3_bucket.blobs.arn}/*"]

    principals {
      type        = "*"
      identifiers = ["*"]
    }

    condition {
      test     = "Bool"
      variable = "aws:SecureTransport"
      values   = ["false"]
    }
  }
}

resource "aws_s3_bucket_policy" "blobs" {
  bucket = aws_s3_bucket.blobs.id
  policy = data.aws_iam_policy_document.blobs.json

  depends_on = [aws_s3_bucket_public_access_block.blobs]
}

# DB。テーブルは 1 つ（#76）。アプリはテーブルを作らない
resource "aws_dynamodb_table" "this" {
  name         = var.name
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "pk"
  range_key    = "sk"

  attribute {
    name = "pk"
    type = "S"
  }

  attribute {
    name = "sk"
    type = "S"
  }

  point_in_time_recovery {
    enabled = true
  }

  deletion_protection_enabled = var.deletion_protection
}
