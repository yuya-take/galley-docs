provider "aws" {
  region = var.region

  default_tags {
    tags = local.tags
  }
}

# CloudFront に付ける ACM の証明書と Lambda@Edge は us-east-1 に作る必要がある
provider "aws" {
  alias  = "us_east_1"
  region = "us-east-1"

  default_tags {
    tags = local.tags
  }
}

data "aws_caller_identity" "current" {}

data "aws_partition" "current" {}
