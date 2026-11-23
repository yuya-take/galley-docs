# ACM の証明書と Route 53 のレコード。
#
# route53_zone_id を渡せば、検証レコードと CloudFront への別名レコードまで作る。渡さなければ、
# 手で作るレコードを outputs（acm_validation_records・dns_records）に出す。ゾーンそのものは作らない。

# CloudFront 用（us-east-1）。画面と資料配信の 2 つのホスト名
resource "aws_acm_certificate" "cloudfront" {
  provider = aws.us_east_1

  domain_name               = var.app_domain
  subject_alternative_names = [local.docs_domain]
  validation_method         = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

# API Gateway のカスタムドメイン用（var.region）。CloudFront から Host を転送するので、画面と同じ名前にする
resource "aws_acm_certificate" "api" {
  domain_name       = var.app_domain
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

locals {
  # 同じアカウントでは、同じホスト名の検証レコードは証明書（リージョン）が違っても同じになるので、ホスト名でまとめる
  acm_validation_records = merge(
    { for o in aws_acm_certificate.api.domain_validation_options : o.domain_name => {
      name  = o.resource_record_name
      type  = o.resource_record_type
      value = o.resource_record_value
    } },
    { for o in aws_acm_certificate.cloudfront.domain_validation_options : o.domain_name => {
      name  = o.resource_record_name
      type  = o.resource_record_type
      value = o.resource_record_value
    } },
  )
  manage_dns = var.route53_zone_id != null
}

resource "aws_route53_record" "acm_validation" {
  for_each = local.manage_dns ? toset([var.app_domain, local.docs_domain]) : toset([])

  zone_id         = var.route53_zone_id
  name            = local.acm_validation_records[each.key].name
  type            = local.acm_validation_records[each.key].type
  records         = [local.acm_validation_records[each.key].value]
  ttl             = 300
  allow_overwrite = true
}

# レコードを手で作るときは、検証が済むまで待つ（既定で最大 75 分）
resource "aws_acm_certificate_validation" "cloudfront" {
  provider = aws.us_east_1

  certificate_arn         = aws_acm_certificate.cloudfront.arn
  validation_record_fqdns = local.manage_dns ? [for r in aws_route53_record.acm_validation : r.fqdn] : null
}

resource "aws_acm_certificate_validation" "api" {
  certificate_arn         = aws_acm_certificate.api.arn
  validation_record_fqdns = local.manage_dns ? [aws_route53_record.acm_validation[var.app_domain].fqdn] : null
}

# 2 つのホスト名を CloudFront に向ける（IPv4 と IPv6）
resource "aws_route53_record" "app" {
  for_each = local.manage_dns ? toset(["A", "AAAA"]) : toset([])

  zone_id = var.route53_zone_id
  name    = var.app_domain
  type    = each.key

  alias {
    name                   = aws_cloudfront_distribution.app.domain_name
    zone_id                = aws_cloudfront_distribution.app.hosted_zone_id
    evaluate_target_health = false
  }
}

resource "aws_route53_record" "docs" {
  for_each = local.manage_dns ? toset(["A", "AAAA"]) : toset([])

  zone_id = var.route53_zone_id
  name    = local.docs_domain
  type    = each.key

  alias {
    name                   = aws_cloudfront_distribution.docs.domain_name
    zone_id                = aws_cloudfront_distribution.docs.hosted_zone_id
    evaluate_target_health = false
  }
}
