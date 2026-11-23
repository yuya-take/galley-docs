# AWS で動かす

AWS では、画面と MCP を Lambda で動かし、資料は S3 から CloudFront で配ります。VPC は作らず、時間課金のサービスを置きません。構成を決めた経緯と比較は PR #75 の本文にあります。

![構成図](architecture.png)

構成図の元ファイルは `architecture.drawio`（draw.io で開いて直せる。AWS のアイコンは `mxgraph.aws4`）です。

AWS で動かすためのアプリの変更は、まだ途中です（#73）。DynamoDB の adapter ができるまでは、下の「Lambda で動かす」の手順は確かめる用途にだけ使えます。

## Lambda で動かす

Docker の構成と同じイメージを、そのまま Lambda のコンテナイメージとして使います。イメージには [Lambda Web Adapter](https://github.com/aws/aws-lambda-web-adapter) が入っていて、Lambda に届いたリクエスト（API Gateway のイベント）を HTTP のリクエストに直してアプリ（:8080）に渡します。アプリは Lambda かどうかを知らず、Docker と同じ HTTP サーバーのまま動きます。

### イメージに入っているもの

- `/opt/extensions/lambda-adapter`：Lambda Web Adapter（版は `Dockerfile` でタグとダイジェストで固定。上げるときは両方を書き換える）。`/opt/extensions` は Lambda の実行環境だけが読むので、`docker run` では動かず、Docker の構成の動きは変わりません
- `AWS_LWA_PORT=8080`：アダプターがリクエストを渡すポート（`GALLEY_APP_ADDR` の既定と同じ）
- `AWS_LWA_READINESS_CHECK_PROTOCOL=tcp`：アダプターは、アプリが待ち受けを始めたかを確かめてから最初のリクエストを渡します。HTTP で確かめると Host が `127.0.0.1:8080` になり、`ALLOWED_HOSTS` を設定していると Host の検査で断られます（403）。アダプターは既定で 499 までを「準備できた」とみなすので動きはしますが、起動のたびに「許可していないホスト名」の警告がログに出るので、TCP の接続だけで確かめます

### Lambda の関数の設定

| 設定 | 値 | 理由 |
| --- | --- | --- |
| パッケージの種類 | コンテナイメージ（ECR） | Lambda のコンテナイメージは ECR からしか取れない |
| アーキテクチャ | `arm64` | イメージのアーキテクチャと合わせる（下の「マルチアーキテクチャのイメージ」） |
| メモリ | 1024 MB | |
| タイムアウト | 30 秒 | API Gateway HTTP API の統合の上限（30 秒）に合わせる |

環境変数（イメージの既定から変えるもの）:

| 環境変数 | 値 | 理由 |
| --- | --- | --- |
| `GALLEY_DATA_DIR` | `/tmp` | Lambda で書き込めるのは `/tmp` だけ。既定の `/data` のままだと DB を作れずに起動に失敗する |
| `ALLOWED_HOSTS` | `galley.example.com` | CloudFront が転送する Host。Host の検査・CSRF の検査・MCP の Host の検査が Docker と同じに通る |
| `GALLEY_APP_URL` | `https://galley.example.com` | MCP が返す資料のリンク |
| `GALLEY_VIEWER_URL` | `https://docs.galley.example.com` | 画面の iframe に入れる資料配信の URL |
| `GALLEY_VIEWER_ADDR` | `127.0.0.1:8081` | 資料配信の待ち受け（下の「資料配信（:8081）」） |
| `GALLEY_MAX_HTML_SIZE` | `4MB` | Lambda の同期の呼び出しはリクエストもレスポンスも 6 MB まで。JSON のエスケープとイベントに包む分の余裕をみる |
| `GALLEY_BLOB_STORE` | `s3://<バケット>/<プレフィックス>` | 資料の実体を S3 に置く。認証は Lambda の実行ロールで行う（`AWS_ACCESS_KEY_ID` などは渡さない） |
| `TZ` | `Asia/Tokyo` | 画面に出す日時のタイムゾーン |

### 資料配信（:8081）

AWS では資料を S3 から CloudFront で配り、アプリの資料配信（:8081）は使いません。止める設定は足さず、動かしたままにします。

- Lambda の実行環境には外から接続する経路が無く、アプリに届くのは Lambda Web Adapter が :8080 に渡すリクエストだけです。:8081 にはどこからも届きません
- 待ち受けたままでも、使うのは空きのソケット 1 つとわずかなメモリだけです。念のため `GALLEY_VIEWER_ADDR=127.0.0.1:8081` にして、実行環境の外向きのインターフェースでは待ち受けないようにします
- `GALLEY_VIEWER_URL` を指定するので、アプリのポートと資料配信のポートを比べる起動時の検査（同じポートなら止める）は効きません。:8080 と :8081 のままにしておけば問題ありません

### 止めるとき

Lambda は実行環境を片付ける前に、アプリ（ランタイム）に SIGTERM を送ります（外部の拡張があるときに限る。Lambda Web Adapter がそれにあたる）。片付けは呼び出しの合間に行われるので処理中のリクエストは無く、アプリは Docker と同じく SIGTERM ですぐに止まります。

### 注意

- **DB はまだ使えない**: DynamoDB の adapter（#76）ができるまで、`GALLEY_DATA_DIR=/tmp` の SQLite は実行環境ごとに別々で、実行環境が片付けられると消えます。手元で確かめる用途にだけ使ってください
- **資料の表示はまだ動かない**: 資料配信の URL を `/b/<hash>` にする変更（#80）と、S3 に書くときの `Content-Type`（PR #75 の 4）が入るまで、S3 から資料を配れません

## マルチアーキテクチャのイメージ

Lambda は、マルチアーキテクチャのイメージ（イメージのインデックス・マニフェストリスト）から関数を作れません。

> Lambda provides multi-architecture base images. However, the image you build for your function must target only one of the architectures. Lambda does not support functions that use multi-architecture container images.
>
> — [Create a Lambda function using a container image](https://docs.aws.amazon.com/lambda/latest/dg/images-create.html)

ghcr.io に出すイメージは、`linux/amd64` と `linux/arm64` をまとめたインデックスです（`.github/workflows/docker.yml` の merge で作る）。ECR に複製するときは、タグ（インデックス）ではなく、インデックスの中の `linux/arm64` のマニフェストだけをダイジェストで指定して複製します。

```bash
# インデックスの中のマニフェストと、それぞれのダイジェストを見る
docker buildx imagetools inspect ghcr.io/yuya-take/galley-docs:<版>
```

ほかに気をつけること:

- インデックスには、来歴の証明（provenance）のマニフェストも `unknown/unknown` のプラットフォームとして入ることがあります。選ぶのは `Platform: linux/arm64` の行のダイジェストです
- 手元で `docker buildx build` でビルドして ECR に push するときも、1 つのアーキテクチャだけをビルドしたうえで、来歴の証明が付いてインデックスにならないように `--provenance=false` を付けます
- 複製の手順そのものは Terraform の Issue（#81）で書きます

## 手元で確かめる（Lambda のエミュレーター）

[aws-lambda-rie](https://github.com/aws/aws-lambda-runtime-interface-emulator)（Lambda Runtime Interface Emulator）で、イメージを Lambda のコンテナイメージとして動かせます。RIE は Lambda の Runtime API と Extensions API を真似るので、`/opt/extensions` の Lambda Web Adapter もそのまま動きます。

1. イメージをビルドする

   ```bash
   docker build -t galley:lambda .
   ```

2. RIE をダウンロードする（arm64 の Mac は `aws-lambda-rie-arm64`、x86_64 は `aws-lambda-rie`）

   ```bash
   mkdir -p ~/.aws-lambda-rie
   curl -fsSL -o ~/.aws-lambda-rie/aws-lambda-rie \
     https://github.com/aws/aws-lambda-runtime-interface-emulator/releases/latest/download/aws-lambda-rie-arm64
   chmod +x ~/.aws-lambda-rie/aws-lambda-rie
   ```

3. RIE を入口にして起動する。RIE は既定で :8080 で呼び出しを受けますが、アプリも :8080 で待ち受けるので、RIE は :9000 にします

   ```bash
   docker run -d --name galley-lambda -p 9000:9000 \
     -v ~/.aws-lambda-rie:/aws-lambda:ro \
     -e GALLEY_DATA_DIR=/tmp \
     -e GALLEY_VIEWER_ADDR=127.0.0.1:8081 \
     --entrypoint /aws-lambda/aws-lambda-rie \
     galley:lambda \
     --runtime-interface-emulator-address 0.0.0.0:9000 /app/galley
   ```

4. API Gateway HTTP API の形のイベント（`event.json`）で呼び出す

   ```json
   {
     "version": "2.0",
     "routeKey": "$default",
     "rawPath": "/",
     "rawQueryString": "",
     "headers": { "host": "localhost", "accept": "text/html" },
     "requestContext": {
       "http": { "method": "GET", "path": "/", "protocol": "HTTP/1.1", "sourceIp": "127.0.0.1", "userAgent": "curl" },
       "domainName": "localhost",
       "requestId": "local",
       "routeKey": "$default",
       "stage": "$default",
       "timeEpoch": 0
     },
     "isBase64Encoded": false
   }
   ```

   ```bash
   curl -sS -d @event.json http://localhost:9000/2015-03-31/functions/function/invocations \
     | jq -r '.statusCode, .headers["content-type"]'
   # 200
   # text/html; charset=utf-8
   ```

   `.body` に画面の HTML が入っています（バイナリのときは `isBase64Encoded` が `true` になり、Base64 で入ります）。ほかの画面やアセットは、`rawPath` と `requestContext.http.path` を変えて呼び出します

5. 片付ける

   ```bash
   docker rm -f galley-lambda
   ```

`docker logs galley-lambda` で、Lambda Web Adapter の起動とアプリのログが見られます。
