# Claude Code Proxy RS

面向 Claude Code 的自托管多账号网关，使用 Rust 构建，提供 Vue 管理端。 (AI&未完成改造版)

**Rust · Tokio · Axum · Vue 3 · PostgreSQL · Redis**

[功能](#功能) · [快速开始](#快速开始) · [Claude Code 接入](#claude-code-接入) · [客户端身份与 TLS](#客户端身份与-tls) · [文档](#文档) · [项目来源](#项目来源)

> [!NOTE]
> 当前为开发预览阶段。OAuth／Setup Token、Messages JSON／SSE、客户端身份及 TLS 模板已实现，并完成本地模拟与管理端验收；真实 Claude 账号联调、Linux 应用镜像和持续负载验证尚未完成。当前使用源码启动，后续工作见 [ROADMAP](ROADMAP.md)。

## 功能

| 能力 | 当前支持 |
| --- | --- |
| Claude Code 账号 | OAuth 授权链接与回填、Setup Token 授权生成或导入、Token 替换、OAuth 手动／自动刷新及重新授权 |
| 原生 Messages | `/v1/messages`，支持 JSON 与 SSE，保留原生请求正文、能力 Header、查询参数及未知字段 |
| 流式转发 | 分段交付、背压、超时与断流处理；客户端取消后释放上游连接及并发租约 |
| 多账号与访问范围 | 账号管理、分组、代理绑定、权重和并发限制；客户端密钥按授权范围访问账号 |
| 客户端接入 | 网关密钥支持 `x-api-key`／Bearer 鉴权；管理端可复制 Claude Code 环境变量配置 |
| 使用记录 | 请求状态、错误、输入／输出及缓存 Token 用量；缺失用量与未配置费用保持未知 |
| 上游客户端身份 | 原生 Claude Code 身份优先；其他客户端使用可配置默认身份，支持平台、密钥及账号覆盖 |
| TLS 模板 | 普通 TLS 或固定版本 Claude Code 模板，在 HTTPS 代理隧道内应用目标侧握手配置 |
| 管理端 | 系统概览、账号、分组、代理、客户端密钥、请求记录及设置页面 |

当前只开放 Claude 产品入口，账号接入方式为 **OAuth 和 Setup Token**。上游账号 Token 与客户端访问网关的密钥分别管理，客户端密钥不会作为上游凭据发送。

自动故障转移、完整订阅额度与计费、Token 计数接口等仍在开发计划中。当前 429／5xx 返回客户端，不自动切换账号重试。

## 快速开始

当前已验证的运行方式是 **本机 Rust 服务 + 前端静态资源 + Docker 中的 PostgreSQL／Redis**。

### 环境要求

- Rust **1.99.1**，由 `rust-toolchain.toml` 固定。
- Node.js：本机验证版本 **24.19.0**；pnpm **11.25.0**。
- Python 3、Docker 和 Docker Compose。
- C/C++ 编译器、CMake、Ninja、Clang／libclang，用于构建 TLS 依赖。

数据库由项目 Compose 启动，无需另外安装数据库服务。完整测试还需要 PostgreSQL 18 客户端工具，详见[开发说明](docs/development.md)。

### 源码启动

~~~sh
git clone https://github.com/zhihaoHappyeveryday/claude-code-proxy-rs.git
cd claude-code-proxy-rs

# 生成本地配置和随机密码，启动 PostgreSQL／Redis，准备测试库
./scripts/dev init

# 安装依赖并构建管理端
pnpm --dir frontend install --frozen-lockfile
./scripts/dev build-web

# 启动网关
./scripts/dev backend
~~~

仓库若仍为私有，需要使用有访问权限的 GitHub 账号克隆。

打开 [http://127.0.0.1:8080](http://127.0.0.1:8080)。初始化生成的管理员用户名和随机密码位于项目根目录 `connections.env`，首次默认用户名为 `admin@claude-proxy.local`。重复初始化保留现有配置和凭据。

| 本地文件 | 用途 |
| --- | --- |
| `.env` | Compose 密码及可选的指定代理出口配置 |
| `connections.env` | 开发／测试连接信息和初始管理员凭据 |
| `deploy/config.yaml` | 网关运行配置 |

这些文件已被 Git 忽略，不应上传到仓库。服务默认仅监听本机回环地址；远程部署说明及当前容器构建边界见[部署说明](deploy/README.md)。

### 配置上游代理

部署时应指定 HTTPS 代理作为 Claude 出口。网关检查账号已绑定代理，且地址与部署指定的出口匹配；未配置出口时，管理端可启动，上游操作会被阻止。

在本地 `.env` 中设置不含账号密码的代理地址，再重启网关。以下仅为格式示例，需替换成自己的指定代理：

~~~dotenv
CPR_CLAUDE_REQUIRED_PROXY_ENDPOINT=https://proxy.example.com:443
~~~

在管理端「代理管理」中创建对应代理并填写认证信息，随后绑定到账号。代理失败不会回退直连。授权浏览器也必须单独配置同一出口；后台代理设置不会自动作用于浏览器。

### 添加账号与客户端密钥

1. 打开「账号管理 → 添加账号」，填写名称并绑定代理。
2. 选择 **OAuth** 或 **Setup Token**。生成授权链接后，在使用相同代理出口的浏览器中完成授权，将完整 `code#state` 或回调 URL 回填；已有 Setup Token 也可以直接粘贴导入。
3. 按需建立账号组，在「API 密钥」创建客户端密钥并配置访问范围。
4. 点击密钥行的「使用密钥」，复制客户端配置。

Setup Token 的“长期有效”不代表永久有效；没有刷新凭据时需要重新授权或替换。模型目录检查与实际生成是不同操作，目录检查失败不能单独证明 Messages 不可用。

## Claude Code 接入

在运行 Claude Code 的终端设置：

~~~sh
export ANTHROPIC_BASE_URL='http://127.0.0.1:8080'
export ANTHROPIC_API_KEY='<管理端生成的客户端密钥>'
unset ANTHROPIC_AUTH_TOKEN
claude
~~~

`ANTHROPIC_BASE_URL` 指向本网关；远程使用时填写网关的 HTTPS 地址。密钥填写网关生成的客户端密钥，不填写上游账号 Token。

其他 Anthropic Messages 客户端可向 `POST /v1/messages` 发起请求，使用 `x-api-key` 或 `Authorization: Bearer` 携带网关密钥。网关会替换上游认证，保留 `anthropic-version`，并在保留原有 `anthropic-beta` 能力值的基础上补充 OAuth／Claude Code 所需标记。

## 客户端身份与 TLS

在「设置 → 上游配置」编辑默认客户端身份和 TLS 模式；账号行「更多 → 账号客户端配置」可选择完整覆盖。生效优先级为：

**账号配置 → 客户端密钥配置 → 平台默认**。客户端密钥的身份覆盖目前通过管理 API 配置。

- **普通 TLS** 为默认模式，使用常规加密连接。
- **模拟模式** 当前提供 Claude Code 2.1.312 / macOS arm64 模板，参考客户端声明 Node.js v26.12.1、SDK 0.128.0，目标侧使用 HTTP/1.1。
- 模板已在本地 HTTPS CONNECT 代理内与实际客户端的归一化 ClientHello 对照；不宣称代理外层握手一致，也不代表已通过真实账号验证。
- 原生 Claude Code 请求保留自身身份；声明不在模板验证范围内时明确拒绝，不静默改版本或回退普通 TLS。
- 配置修改只影响新请求，在途 SSE 保持原配置；请求详情展示实际来源、模板版本和执行阶段。

界面中的 Node.js 版本是出站身份声明，不要求网关服务器安装相同版本，也不会仅通过修改这个数字改变 TLS 握手。

## 文档

| 任务 | 文档 |
| --- | --- |
| 开发环境、架构、验证与测试 | [开发说明](docs/development.md) |
| 本机配置、服务管理与镜像构建 | [部署说明](deploy/README.md) |
| 数据库变更与迁移规则 | [数据库迁移](backend/migrations/README.md) |
| 后续功能与验收目标 | [ROADMAP](ROADMAP.md) |

## 项目来源

- [zyycn/codex-proxy-rs](https://github.com/zyycn/codex-proxy-rs)：后端分层、通用管理能力与管理端设计的主要代码来源；继承基点为 005466bcb0f5154f36f495cd8cd34ac267ceaf31。
- [Wei-Shaw/sub2api](https://github.com/Wei-Shaw/sub2api)：Claude 专项协议、账号授权及 TLS 机制的设计参考，相关适配在 Rust 中独立实现。
- [zyycn/codex-proxy-ui](https://github.com/zyycn/codex-proxy-ui)：管理端使用的 UI 组件库，依赖版本由 pnpm 锁文件记录。

README 的信息组织参考了 codex-proxy-rs 的项目首页，功能与运行步骤以本项目实际实现为准。

## 许可证

项目采用 [Apache License 2.0](LICENSE)，保留继承代码的许可证及来源声明。修改归属见 [NOTICE](NOTICE)；第三方依赖遵循各自许可证。
