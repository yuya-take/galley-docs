# Cognito。ブラウザーのログイン（Lambda@Edge）と、MCP クライアントの OAuth（動的なクライアント登録）に使う。
#
# 動的なクライアント登録（DCR）は、AWS プロバイダーがまだ設定できない（UpdateUserPool の DcrConfiguration）。
# apply のあとに、README の手順でコンソールから有効にする。Terraform がこのプールを更新しても、
# DcrConfiguration は送らないので、コンソールで入れた設定は消えない。

resource "aws_cognito_user_pool" "this" {
  name = var.name

  # 動的なクライアント登録は Essentials 以上
  user_pool_tier      = "ESSENTIALS"
  deletion_protection = var.deletion_protection ? "ACTIVE" : "INACTIVE"

  # メールアドレスでログインする。利用者は管理者が作り、自分では登録させない
  username_attributes      = ["email"]
  auto_verified_attributes = ["email"]

  username_configuration {
    case_sensitive = false
  }

  admin_create_user_config {
    allow_admin_create_user_only = true

    invite_message_template {
      email_subject = "Galley の招待"
      email_message = "Galley（${local.app_origin}/）に招待されました。ユーザー名は {username}、仮のパスワードは {####} です。最初のログインでパスワードを変えてください。"
      sms_message   = "Galley のユーザー名は {username}、仮のパスワードは {####} です。"
    }
  }

  account_recovery_setting {
    recovery_mechanism {
      name     = "verified_email"
      priority = 1
    }
  }

  password_policy {
    minimum_length                   = 12
    require_lowercase                = true
    require_uppercase                = true
    require_numbers                  = true
    require_symbols                  = false
    temporary_password_validity_days = 7
  }

  mfa_configuration = var.mfa_configuration

  dynamic "software_token_mfa_configuration" {
    for_each = var.mfa_configuration == "ON" ? [1] : []
    content {
      enabled = true
    }
  }
}

# マネージドログイン（version 2）のドメイン。DCR はすべてのドメインがマネージドログインであることを求める。
# ログインの Cookie は app_domain の下のすべてに届くので、カスタムドメインは使わず、プレフィックスのドメインにする
resource "aws_cognito_user_pool_domain" "this" {
  domain                = var.cognito_domain_prefix
  user_pool_id          = aws_cognito_user_pool.this.id
  managed_login_version = 2
}

# ブラウザーのログイン用。公開クライアント（シークレットなし、PKCE）にして、Lambda@Edge に秘密を持たせない
resource "aws_cognito_user_pool_client" "browser" {
  name         = "${var.name}-browser"
  user_pool_id = aws_cognito_user_pool.this.id

  generate_secret                      = false
  allowed_oauth_flows_user_pool_client = true
  allowed_oauth_flows                  = ["code"]
  allowed_oauth_scopes                 = ["openid", "email", "profile"]
  supported_identity_providers         = ["COGNITO"]
  callback_urls                        = ["${local.app_origin}/_auth/callback"]
  logout_urls                          = ["${local.app_origin}/"]
  explicit_auth_flows                  = ["ALLOW_USER_SRP_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]

  prevent_user_existence_errors = "ENABLED"
  enable_token_revocation       = true

  id_token_validity      = 60
  access_token_validity  = 60
  refresh_token_validity = local.session_days

  token_validity_units {
    id_token      = "minutes"
    access_token  = "minutes"
    refresh_token = "days"
  }
}

# マネージドログインの見た目（Cognito の既定）。動的に登録されたクライアントは、スタイルが無ければ既定の見た目になる
resource "aws_cognito_managed_login_branding" "browser" {
  user_pool_id                = aws_cognito_user_pool.this.id
  client_id                   = aws_cognito_user_pool_client.browser.id
  use_cognito_provided_values = true
}

# MCP サーバーを表すリソースサーバー。MCP クライアントが resource に指定する URL を識別子にし、
# このスコープを DCR の「許可するスコープ」に入れる。アクセストークンの aud がこの URL になる
resource "aws_cognito_resource_server" "mcp" {
  user_pool_id = aws_cognito_user_pool.this.id
  identifier   = local.mcp_resource
  name         = "${var.name}-mcp"

  scope {
    scope_name        = "access"
    scope_description = "Galley の MCP のツールを使う"
  }
}
