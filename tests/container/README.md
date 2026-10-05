# Container Tests for mihomo-cli

容器测试用于验证 sudo 提权、TUN 配置隔离、路径解析等跨平台功能。

## 快速开始

同一入口支持 Linux Docker Engine，以及 macOS 上的 Docker Desktop 或 Colima。
运行器会使用当前 Docker context；macOS context 不可用时会自动发现 Colima socket。
镜像内完成 Linux 编译，不依赖宿主机 `target/release`，因此支持 Intel/ARM macOS
和 x86_64/aarch64 Linux。

```bash
# macOS 使用 Colima 时
colima start

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

# 推送前完整检查
just pre-push-check
```

`just test-container-required <test...>` 用于 CI：Docker 不可用或测试被跳过均视为失败。
`just test-systemd-contract` 使用独立 systemd/privileged 镜像严格验证 system service
安装、路径解析、daemon/Core 生命周期、终端/系统代理、autostart、TUN 状态和
`mihomo -t` runtime validation。
`just test-tun-config-contract` 保留为该完整合约的兼容别名。
`just quick-test` 是本机和 CI 的完整单发行版入口：它先执行 runner 自检和全部离线
smoke/journey 测试，再执行上述严格 systemd 合约。发行版矩阵仍由
`just test-container --matrix` 单独执行。需要真实代理节点的外部路由旅程尚未具备可信
fixture，因此不在容器测试清单中。

## 显式真实订阅 Core 数据面 probe

`tests/manual/test-real-subscription-core-dataplane.sh` 不属于 CI 或 `quick-test`。它只在用户明确
提供真实订阅、确认外部网络验证时手工运行：

```bash
CLASH_CONFIG_URL='...' \
MIHOMO_REAL_CORE=/path/to/real/mihomo \
tests/manual/test-real-subscription-core-dataplane.sh
```

脚本启动一次性 privileged Docker 容器，挂载当前 CLI 和真实 Core 为只读文件；订阅 URL
仅作为容器环境变量使用。它在容器临时目录中验证订阅转换、真实 Core 校验、非 TUN proxy
数据面，以及真实 TUN API/interface/存活连接。不得把订阅 URL、转换后的配置、节点、token
或出口 IP 输出、保存或提交。它不执行或验证 `mihomo-cli install`、daemon、restart、`tun on`
或 `status`，因此不验证 system service 重装用户旅程。

## 测试产物与 glibc ABI 守卫

所有容器套件（runner、`quick-test`、systemd 合约、user-mode 合约）统一消费
`target/container-test/{mihomo-cli,mihomo}`。产物准备方式由环境变量 `MIHOMO_ARTIFACTS` 控制：

| 取值 | 行为 | 适用场景 |
| --- | --- | --- |
| （不设） | 宿主机是 Linux/x86_64 时宿主编译，否则容器内编译 | 单镜像快速验证 |
| `container` | 强制 `rust:slim-bullseye` 容器内编译 | `--matrix` 自动设置：老发行版镜像 glibc 低于宿主，宿主产物会报 `GLIBC_x.xx not found` |
| `existing` | 不编译，复用已就位产物（缺失即失败） | 用 musl 静态产物或指定构建跑测试 |

**glibc ABI 守卫**：每个镜像构建后、测试执行前，运行器用 `objdump -T` 读取产物最高
`GLIBC_x.xx` 符号需求，并与镜像内 `ldd --version` 的实际 glibc 比较；产物需求更高时直接
判该镜像失败并跳过其全部用例（避免在注定 `GLIBC_x.xx not found` 的环境里得到不可信结果）。
静态 musl 产物无 GLIBC 符号，自动通过。

```bash
# 用 musl 静态产物跑全量离线测试（消除 glibc 版本耦合）
cargo zigbuild --target x86_64-unknown-linux-musl --release
cp target/x86_64-unknown-linux-musl/release/mihomo-cli target/container-test/mihomo-cli
MIHOMO_ARTIFACTS=existing just test-container --all
```


## 测试场景

| 场景 | 验证点 | 容器要求 |
|------|--------|----------|
| sudo HOME 路径 | 提权后配置路径指向原始用户 | 普通用户 + sudo |
| TUN 配置隔离 | daemon 接受 tun-config.yaml | systemd + 非 root daemon，TUN 通过 root peer gate |
| 配置文件所有权 | sudo 写入后 chown 回原始用户 | 普通用户 + sudo |
| 显式 sudo | SUDO_UID 还原原始用户 | 普通用户 + sudo |

---

## 开发者指南：如何添加新测试

### 步骤 1：创建测试定义文件

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

### 步骤 2：创建测试脚本

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

### 步骤 3：验证测试

```bash
# 列出测试，确认新测试被发现
just test-container list

# 运行新测试
just test-container my-feature
```

### 完整示例

假设要添加一个"网络连通性"测试：

**1. 测试定义** (`tests/container/tests/network.test`)：
```bash
NAME=network
DESC=验证容器内网络连通性
IMAGE=ubuntu:24.04
SCRIPT=test-network.sh
TAGS=(network, basic)
```

**2. 测试脚本** (`tests/container/scripts/test-network.sh`)：
```bash
#!/bin/bash
set -e

echo "测试网络连通性..."

# 测试 DNS 解析
if nslookup google.com >/dev/null 2>&1; then
    echo "✅ DNS 解析正常"
else
    echo "❌ DNS 解析失败"
    exit 1
fi

# 测试 HTTP 连接
if curl -s --max-time 5 https://google.com >/dev/null; then
    echo "✅ HTTP 连接正常"
else
    echo "❌ HTTP 连接失败"
    exit 1
fi

echo "✅ 网络测试通过"
```

**3. 运行测试**：
```bash
just test-container network
```

---

## 文件结构

```
tests/container/
├── README.md              # 本文档
├── config.toml            # 配置文件（默认镜像、矩阵镜像）
├── test-runner.sh         # 普通容器测试运行器
├── quick-test.sh          # 严格聚合入口
├── systemd-test.sh        # systemd/privileged 合约入口
├── Dockerfile.simple      # 普通离线测试镜像
├── Dockerfile.tun-contract # systemd 合约镜像
├── tests/                 # 普通容器测试定义
└── scripts/               # 普通测试与 systemd 合约脚本
```

## 配置说明

`config.toml` 配置项：

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

## 环境要求

- Docker 已安装并运行
- Linux/macOS（Windows 需要 WSL）
- TUN 测试需要 `--privileged` 模式
