variable "app_domain" {
  description = "画面と MCP のホスト名（例: galley.example.com）。資料配信は docs.<app_domain> になる。ログインの Cookie はこのドメインとその下のすべてに届くので、この下には Galley のホストだけを置く"
  type        = string

  validation {
    condition     = can(regex("^([a-z0-9]([a-z0-9-]*[a-z0-9])?\\.)+[a-z]{2,}$", var.app_domain))
    error_message = "app_domain は小文字のホスト名で指定してください（例: galley.example.com）。"
  }
}

variable "cognito_domain_prefix" {
  description = "Cognito のマネージドログインのドメインの接頭辞。<接頭辞>.auth.<region>.amazoncognito.com になる。リージョンの中で一意にする（例: acme-galley）"
  type        = string

  validation {
    condition     = can(regex("^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$", var.cognito_domain_prefix)) && !can(regex("aws|amazon|cognito", var.cognito_domain_prefix))
    error_message = "cognito_domain_prefix は英小文字・数字・ハイフンで指定してください。aws・amazon・cognito を含む名前は Cognito が受け付けません。"
  }
}

variable "image_tag" {
  description = "ECR に複製したイメージのタグ（例: v0.1.0）。最初の apply（ECR と証明書だけを作る段階）では未設定でよい"
  type        = string
  default     = null
}

variable "route53_zone_id" {
  description = "Route 53 のホストゾーンの ID。渡せば ACM の検証レコードと CloudFront への別名レコードまで作る。渡さなければ、手で作るレコードを出力する（ゾーンそのものは作らない）"
  type        = string
  default     = null
}

variable "name" {
  description = "リソースの名前に付ける接頭辞"
  type        = string
  default     = "galley"

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,30}$", var.name))
    error_message = "name は英小文字で始まる、英小文字・数字・ハイフンの 2〜31 文字にしてください。"
  }
}

variable "region" {
  description = "Lambda・API Gateway・DynamoDB・S3・Cognito を置くリージョン"
  type        = string
  default     = "ap-northeast-1"
}

variable "mfa_configuration" {
  description = "ログインの多要素認証。OFF（使わない）か ON（認証アプリの TOTP を全員に求める。初回のログインで設定させる）"
  type        = string
  default     = "OFF"

  validation {
    condition     = contains(["OFF", "ON"], var.mfa_configuration)
    error_message = "mfa_configuration は OFF か ON にしてください。"
  }
}

variable "deletion_protection" {
  description = "Cognito のユーザープールと DynamoDB のテーブルの削除保護。S3 のバケットも、true のあいだは中身があると消さない。destroy の前に false にして apply する"
  type        = bool
  default     = true
}

variable "noncurrent_version_retention_days" {
  description = "S3 で上書き・削除された資料の実体（古い版）を残す日数。物理削除（galley purge）した資料も、この日数のあいだは S3 の古い版として残る"
  type        = number
  default     = 30

  validation {
    condition     = var.noncurrent_version_retention_days >= 1
    error_message = "noncurrent_version_retention_days は 1 以上にしてください。"
  }
}

variable "blob_prefix" {
  description = "S3 のバケットの中で資料の実体を置くプレフィックス"
  type        = string
  default     = "blobs"

  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9/_-]*$", trim(var.blob_prefix, "/")))
    error_message = "blob_prefix は英小文字・数字・ハイフン・アンダースコア・スラッシュで指定してください。"
  }
}

variable "bucket_name" {
  description = "資料の実体を置く S3 のバケットの名前。未設定なら <name>-blobs-<アカウント ID> にする"
  type        = string
  default     = null
}

variable "max_html_size" {
  description = "1 ファイルの HTML の上限（GALLEY_MAX_HTML_SIZE）。Lambda の同期の呼び出しは 6 MB までなので、余裕を見て 4MB にする"
  type        = string
  default     = "4MB"
}

variable "timezone" {
  description = "画面に出す日時のタイムゾーン（TZ）"
  type        = string
  default     = "Asia/Tokyo"
}

variable "app_environment" {
  description = "アプリの Lambda に足す環境変数。同じ名前があれば、こちらが優先される"
  type        = map(string)
  default     = {}
}

variable "price_class" {
  description = "CloudFront の価格クラス。PriceClass_200 は日本を含む"
  type        = string
  default     = "PriceClass_200"
}

variable "api_throttling_rate_limit" {
  description = "API Gateway の 1 秒あたりのリクエストの上限（定常）"
  type        = number
  default     = 50
}

variable "api_throttling_burst_limit" {
  description = "API Gateway の 1 秒あたりのリクエストの上限（瞬間）"
  type        = number
  default     = 100
}

variable "log_retention_days" {
  description = "CloudWatch Logs の保存日数"
  type        = number
  default     = 30
}

variable "origin_verify_previous_values" {
  description = "秘密のヘッダー（X-Origin-Verify）を入れ替えるあいだだけ、前の値をここに入れて一時的に通す（README の「秘密のヘッダーの入れ替え」）"
  type        = list(string)
  default     = []
  sensitive   = true
}

variable "tags" {
  description = "すべてのリソースに付けるタグ"
  type        = map(string)
  default     = {}
}
