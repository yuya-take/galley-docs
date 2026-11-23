'use strict';

// config.json は Terraform（edge.tf）が作ってパッケージに入れる。Lambda@Edge は環境変数を使えないため。
const config = require('./config.json');
const { createHandler } = require('./handler');

exports.handler = createHandler(config);
