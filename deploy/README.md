# 部署与运行

当前已验证的是本机 Rust 进程提供 API 与管理端静态页面，连接项目独立 Docker PostgreSQL／Redis。源码启动步骤见 [README](../README.md#快速开始)，构建和测试依赖见 [开发说明](../docs/development.md)。

## 配置与服务

首次执行 `./scripts/dev init` 会生成私有配置、启动数据服务并创建测试库。开发 Compose 使用以下地址：

| 服务 | 宿主机地址 | 用途 |
| --- | --- | --- |
| PostgreSQL | `127.0.0.1:15432` | `claude_code_proxy` 开发库及 `claude_code_proxy_test` 测试库 |
| Redis | `127.0.0.1:16379` | DB0 开发状态；DB15 测试状态 |
| 网关 | `127.0.0.1:8080` | API 和已构建的管理页面 |

`deploy/compose.dev.yaml` 只启动数据服务，不会启动应用容器。它的容量限制用于开发环境，不代表生产配置或性能承诺。

```sh
./scripts/dev-services ps
./scripts/dev-services stop
./scripts/dev-services up -d --wait
```

上述停止／启动保留命名卷中的数据。正常数据与测试数据应分开，测试 Redis 库不能指向开发 DB0。

## 数据库与凭据

- `.env` 保存 Compose 密码，`connections.env` 保存开发／测试连接信息和初始管理员凭据。
- `deploy/config.yaml` 由配置模板生成；重复初始化保留已有配置。
- 初始密码随机生成，以上文件均不提交 Git。修改 `.env` 不会自动修改已初始化数据库内的密码。
- 手工配置 PostgreSQL／Redis 密码时，应用要求 16–128 字节、不能全为空白或含控制字符，同时应满足对应服务策略；初始管理员密码至少 12 个字符且不能包含 `$`。
- 应用启动时在监听请求之前执行 SQLx 迁移，规则见 [数据库迁移](../backend/migrations/README.md)。
- `.runtime/` 包含运行数据和日志。备份恢复还必须考虑配置及解密凭据需要的材料，完整发布恢复流程尚待验收。

## 指定代理出口

通过本地 `.env` 或进程环境设置 `CPR_CLAUDE_REQUIRED_PROXY_ENDPOINT`，值为不含账号密码的 HTTPS 代理地址。管理端配置对应代理认证，并将账号绑定到该出口。

`scripts/dev backend` 自动读取本地 `.env` 中的该项；直接启动二进制或容器时应显式传入。缺少配置会阻止 Claude 上游访问，代理故障不会回退直连。浏览器 OAuth 授权需要单独配置相同出口。

## 远程访问

默认监听回环地址。远程使用需要根据部署拓扑配置 HTTPS 反向代理，转发到应用端口；SSE 路由需关闭响应缓冲，并为长连接设置合适的读取超时。数据库和 Redis 不应作为客户端访问入口。

`api.request_timeout_seconds` 为应用总请求期限，默认 `null`；设置正数后同时覆盖 JSON 和 SSE。模型目录检查使用独立的短超时，不能用它代替长流期限配置。

## 源码镜像构建

仓库提供多阶段 Dockerfile：构建前端、构建 Rust 服务，再使用非 root 用户运行。镜像内包含独立静态资源目录。

```sh
docker build -f deploy/Dockerfile --target runtime -t claude-code-proxy-rs:local .
```

**Linux 应用镜像尚未实测构建和运行。** 当前没有已验证的应用 Compose 安装流程或正式发布镜像。本命令是待验证的源码构建入口，不能将构建模板的存在当成部署验收通过。

容器运行仍需配置数据库／Redis 网络、私有配置、运行数据挂载、监听地址、静态资源目录与指定代理出口。相关工作见 [ROADMAP](../ROADMAP.md)。
