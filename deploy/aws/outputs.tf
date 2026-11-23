output "app_url" {
  description = "画面の URL"
  value       = "${local.app_origin}/"
}

output "mcp_url" {
  description = "MCP クライアントに設定する URL"
  value       = local.mcp_resource
}

output "docs_url" {
  description = "資料配信の URL"
  value       = "${local.docs_origin}/"
}

output "ecr_repository_url" {
  description = "イメージを複製する ECR のリポジトリ"
  value       = aws_ecr_repository.app.repository_url
}

output "acm_validation_records" {
  description = "route53_zone_id を渡さないときに、手で作る ACM の検証レコード（CNAME）"
  value       = local.manage_dns ? [] : distinct(values(local.acm_validation_records))
}

output "dns_records" {
  description = "route53_zone_id を渡さないときに、手で作る CloudFront へのレコード（ゾーンの頂点なら CNAME ではなく ALIAS・ANAME にする）"
  value = local.manage_dns ? [] : [
    { name = var.app_domain, type = "CNAME", value = aws_cloudfront_distribution.app.domain_name },
    { name = local.docs_domain, type = "CNAME", value = aws_cloudfront_distribution.docs.domain_name },
  ]
}

output "cloudfront_distribution_ids" {
  description = "CloudFront のディストリビューションの ID（docs は物理削除のあとのキャッシュの無効化に使う）"
  value = {
    app  = aws_cloudfront_distribution.app.id
    docs = aws_cloudfront_distribution.docs.id
  }
}

output "cognito_user_pool_id" {
  description = "Cognito のユーザープールの ID（利用者を作るときに使う）"
  value       = aws_cognito_user_pool.this.id
}

output "cognito_issuer" {
  description = "Cognito の発行元（API Gateway の JWT オーソライザーと、MCP の保護されたリソースのメタデータに入れたもの）"
  value       = local.cognito_issuer
}

output "cognito_login_domain" {
  description = "Cognito のマネージドログインのドメイン"
  value       = "https://${local.cognito_login_domain}"
}

output "mcp_scope" {
  description = "動的なクライアント登録の「許可するスコープ」に入れるスコープ"
  value       = local.mcp_scope
}

output "dynamodb_table_name" {
  description = "DynamoDB のテーブル（GALLEY_DB=dynamodb://<テーブル>）"
  value       = aws_dynamodb_table.this.name
}

output "blob_store" {
  description = "資料の実体の置き場所（GALLEY_BLOB_STORE）"
  value       = "s3://${aws_s3_bucket.blobs.bucket}/${local.blob_prefix}"
}

output "blob_bucket" {
  description = "資料の実体を置く S3 のバケット"
  value       = aws_s3_bucket.blobs.bucket
}

output "origin_verify_value" {
  description = "CloudFront が API Gateway に付ける秘密のヘッダーの今の値（入れ替えのときに使う）"
  value       = random_password.origin_verify.result
  sensitive   = true
}
