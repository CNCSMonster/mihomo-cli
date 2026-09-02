# CONTRIBUTING

> mihomo-cli 贡献指南

---

## 开发环境

```bash
git clone <repo-url>
cd mihomo-cli
cargo build
cargo test
```

- Rust 1.80+
- 推荐使用 `rust-analyzer`（IDE 支持）

## 提交前检查清单

```bash
cargo test                             # 全量测试
cargo clippy --all-targets             # Lint 检查
cargo build --release                  # Release 构建验证
```

所有检查通过后再提交。

## 项目结构

```
src/
├── main.rs          CLI 入口 + 命令路由 (clap)
├── mihomo_api.rs    Unix socket REST 客户端
├── config.rs        订阅管理 + 配置生成
├── installer.rs     核心二进制下载 + Geo 文件管理
├── service.rs       系统服务执行层 + 提权 (systemd/LaunchDaemon)
├── instance.rs      Instance Model：路径矩阵 + 模式解析 + service plan
├── daemon.rs        daemon 进程 (IPC + readiness + lifecycle 串行化)
├── ipc.rs           daemon IPC 客户端
├── lock.rs          并发锁
├── rules.rs         用户路由规则管理
├── dns.rs           DNS 路由策略管理
├── backup.rs        配置备份与恢复
├── system_proxy.rs  系统代理设置 (macOS/Linux)
├── ui.rs            交互式 TUI (crossterm)
├── yaml_editor.rs   serde_yaml 校验 + 标记区块编辑（ADR-10）
└── utils.rs         路径/工具函数
```

## 文档体系

| 文档 | 说明 |
|------|------|
| [README.md](../../README.md) | 项目入口 |
| [USAGE.md](../../USAGE.md) | 命令参考 |
| [SPEC.md](../../SPEC.md) | 软件设计 |
| [CONTEXT.md](../../CONTEXT.md) | 领域知识与术语 |
| [ROADMAP.md](../../ROADMAP.md) | 规划与 Bug |
| [CHANGELOG.md](../../CHANGELOG.md) | 变更记录 |

修改功能时，同步更新对应的文档。

### docs/ 目录结构

```text
docs/
├── ci/                 CI 与覆盖率说明
├── contributing/       贡献指南
├── testing/            测试说明
├── SECURITY.md         安全模型与边界
├── architecture.md     架构说明
├── SPEC-*.md           专项规格说明
└── user-journeys.md    用户旅程与证据边界
```

设计决策应写入公开可访问的正式文档；历史版本由 Git 记录，不创建或引用未发布的内部归档。

## 代码风格

- 遵循 `cargo fmt` 和 `cargo clippy` 默认规则
- 函数/模块注释用中文
- 日志用 `crate::log!()` 宏（可通过 `-v` / `--verbose` 启用）
- API 端点路径变更时，核实 Mihomo 官方文档及公开的兼容客户端实现

## 提交流程

1. 从 `main` 拉取最新代码
2. 创建功能分支
3. 实现功能并运行测试与 Lint
4. 提交符合项目约定的变更
5. Push 功能分支到个人 fork
6. 向公开仓库提交 Pull Request

## 第三方参考

- [clash-verge-rev](https://github.com/clash-verge-rev/clash-verge-rev) — GUI 客户端，Mihomo API 用法参考
- [mihomo-cli (TS)](https://github.com/adaex/mihomo-cli) — TypeScript CLI 参考实现
## 本地覆盖率检查

```bash
# 默认最低覆盖率阈值：23%
scripts/coverage.sh

# 指定最低覆盖率阈值
scripts/coverage.sh 25
```

脚本会运行 `cargo tarpaulin --ignore-tests`，并生成：

- `target/coverage/tarpaulin-report.html`
- `target/coverage/cobertura.xml`

如果本机没有安装 tarpaulin：

```bash
cargo install cargo-tarpaulin
```

## 容器测试

容器测试用于验证 sudo 提权、TUN 配置隔离、路径解析等需要隔离环境的功能。
同一入口支持 Linux Docker Engine 与 macOS Docker Desktop/Colima。macOS 推荐先运行
`colima start`；当前 Docker context 失效时，运行器会自动发现 Colima socket。Linux
二进制在容器 builder 内按目标架构编译，不依赖宿主机产物。

### 快速开始

```bash
# 列出所有测试
just test-container list

# 运行所有测试
just test-container

# 完整单发行版验证：runner 自检 + 全部离线测试 + systemd 合约
just quick-test

# 运行指定测试
just test-container sudo-context

# 矩阵测试（多镜像）
just test-container --matrix

# Linux systemd 合约（macOS Docker Desktop/Colima 与 Linux Docker Engine 均可）
just test-systemd-contract

# 推送前完整检查
just pre-push-check
```

`just quick-test` 是本机与 CI 共用的严格聚合入口；它不包含发行版矩阵。需要真实代理
节点的外部路由旅程尚未具备可信 fixture，因此不在容器测试清单中。shell/system proxy、
autostart、service lifecycle、路径解析与 `tun` 由 `just test-systemd-contract` 在
privileged systemd 容器中统一验证。

### 添加新测试

**步骤 1：创建测试定义文件**

在 `tests/container/tests/` 下创建 `.test` 文件：

```bash
# tests/container/tests/my-feature.test
NAME=my-feature
DESC=验证我的新功能
IMAGE=ubuntu:24.04
SCRIPT=test-my-feature.sh
TAGS=(feature, linux)
REQUIRES=  # 可选：privileged, network, tun
```

**字段说明：**

| 字段 | 必填 | 说明 |
|------|------|------|
| `NAME` | ✅ | 测试名称（用于命令行调用） |
| `DESC` | ✅ | 测试描述（显示在 list 中） |
| `IMAGE` | ✅ | Docker 镜像（默认 ubuntu:24.04） |
| `SCRIPT` | ✅ | 测试脚本路径（相对于 scripts/） |
| `TAGS` | ❌ | 标签（用于过滤） |
| `REQUIRES` | ❌ | 依赖（privileged/network/tun） |

**步骤 2：创建测试脚本**

在 `tests/container/scripts/` 下创建测试脚本：

```bash
# tests/container/scripts/test-my-feature.sh
#!/bin/bash
set -e

echo "测试我的新功能..."

# 测试逻辑
if some_check; then
    echo "✅ 检查通过"
else
    echo "❌ 检查失败"
    exit 1
fi
```

**脚本规范：**
- 以 `#!/bin/bash` 开头
- 使用 `set -e` 遇到错误立即退出
- 成功输出 `✅`，失败输出 `❌` 并 `exit 1`
- 可以假设在容器内运行，有 root 权限

**步骤 3：验证测试**

```bash
# 列出测试，确认新测试被发现
just test-container list

# 运行新测试
just test-container my-feature
```

### 配置说明

`tests/container/config.toml` 配置项：

```toml
# 默认镜像（本地快速测试用）
default_image = "ubuntu:24.04"

# 矩阵测试镜像列表
matrix_images = [
    "ubuntu:24.04",
    "ubuntu:22.04",
    "debian:12",
]
```

### 环境要求

- Docker 已安装并运行
- Linux/macOS（Windows 需要 WSL）
- TUN 测试需要 `--privileged` 模式


## CI and coverage

Before pushing, run the normal test suite:

```bash
cargo test
```

Coverage baseline is enforced in CI with:

```bash
scripts/coverage.sh 23
```

The baseline and rationale are documented in `docs/ci/coverage-baseline.md`.

## 订阅 User-Agent 贡献边界

`mihomo-cli config add/refresh/probe` 的 UA 协商只面向 Clash/Mihomo 兼容订阅格式，目标是尽可能获取供应商原始 Clash YAML，保留其 `proxy-groups`、`rules`、`proxy-providers` 和 `rule-providers`。

当前默认候选应保持“小集合、Clash-compatible、成功即停”：

- `clash-verge/...`
- `clash-meta/...`
- `clash/...`

不要把 Surge、Quantumult X、Shadowrocket、v2rayN、sing-box 等非 Clash 生态 UA 加入默认候选。原因：

1. 默认添加/刷新订阅不应对带 token 的订阅 URL 发送过多请求，避免触发服务端限流；
2. 非 Clash UA 往往返回其它配置格式或 raw links，不是 mihomo-cli 当前承诺支持的配置来源；
3. 从 raw links 反推出供应商分流规则不可可靠实现，不能根据节点名猜规则。

如果需要新增 Clash-compatible UA 候选，请同时满足：

- 该 UA 在主流订阅面板中明确用于返回 Clash/Mihomo YAML；
- 不增加默认请求数量的量级，仍保持 bounded auto negotiation；
- 更新 `subscription_ua_candidates_cover_common_clash_clients` 等相关测试；
- 更新 `README.md`、`README_en.md`、`USAGE.md` 中的 UA 边界说明；
- 不引入根据节点名、地区名、营销标签自动生成路由规则的行为。

支持其它订阅生态（例如 Surge/Quantumult X/Shadowrocket）应作为单独功能设计，不应混入默认 Clash-compatible UA 探测。
