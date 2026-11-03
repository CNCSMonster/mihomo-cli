# mihomo-cli 任务管理器
# 用法: just <command> [args]

# 可选导入内部开发任务（仅开发仓存在，公开仓静默忽略）
import? "justfile.internal"

# 默认命令：完整 lint
default:
    just lint

# ━━━ 环境初始化 ━━━

# 初始化开发环境（首次 clone 后运行）
# 用法:
#   just init              # 自动：根据当前平台安装合适的 targets + 工具
#   just init all          # 安装所有 targets
#   just init windows      # 仅 Windows 交叉编译
#   just init macos        # 仅 macOS 交叉编译
#   just init linux        # 仅 Linux 交叉编译
#   just init tools        # 仅安装开发工具
init profile="auto":
    scripts/init.sh {{profile}}

# ━━━ 质量检查 ━━━

# 完整 lint（根据当前平台检查可编译的 targets + fmt + test）
# 用法:
#   just lint              # 自动：根据当前平台检查可编译的 targets
#   just lint full         # 完整：包括需要特殊工具链的 targets
#   just lint windows      # 仅 Windows targets
#   just lint macos        # 仅 macOS targets
#   just lint linux        # 仅 Linux targets
#   just lint native       # 仅 native target
lint scope="auto":
    scripts/lint.sh {{scope}}

# 快速检查（native fmt + clippy，不跑 test，迭代用）
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings

# 检查指定 target（如 just check-target x86_64-pc-windows-gnu）
check-target TARGET:
    cargo clippy --target {{TARGET}} -- -D warnings

# ━━━ 测试 ━━━

# 运行全部单元测试
test:
    cargo test

# 运行容器测试（验证 sudo 提权、路径解析等）
# 用法:
#   just test-container                  # 运行所有可用测试
#   just test-container list             # 列出所有可用测试
#   just test-container sudo-context     # 运行指定测试
#   just test-container --matrix         # 矩阵测试（多镜像）
#   just test-container --image=debian:12  # 指定镜像
#   just test-container --parallel       # 并行执行
test-container *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    bash tests/container/test-runner.sh {{ARGS}}

# 运行容器测试（严格模式：skip = fail）
# 用法:
#   just test-container-required j001-offline-subscription   # 强制运行指定测试
# TUN 需要 systemd 专用镜像；在该入口完成前不得作为 required 测试运行。
test-container-required *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    bash tests/container/test-runner.sh --required {{ARGS}}

# Runner 自身语义验证（无需 Docker）
test-runner-self-test:
    bash tests/container/test-runner.sh --self-test

# 完整 systemd 合约的兼容别名
test-tun-config-contract:
    bash tests/container/systemd-test.sh

# 代际升级应用 E2E（systemd + fake core，privileged）
test-generation-e2e:
    bash tests/container/generation-apply-test.sh

# Legacy migration, explicit adoption/revocation, and upgrade with a real Core
test-generation-migration-adoption:
    bash tests/container/generation-migration-adoption-test.sh

# 严格验证 Linux system service 安装、路径、生命周期、proxy、autostart、TUN 和 runtime validation
test-systemd-contract:
    bash tests/container/systemd-test.sh

# 严格验证 Linux per-user service、Core 自行安装、启动、重启和卸载
test-user-mode:
    bash tests/container/user-mode-test.sh

# 使用同架构真实 Mihomo Core 验证 system install/restart/TUN/runtime/cleanup；缺少真实 Core 时失败
real-core-systemd-test:
    bash tests/container/real-core-systemd-test.sh

# 真实 Core 终审： 按订阅选择持久化 + TUN-active promotion，覆盖普通用户与 root 两种身份
real-core-select-promotion-test:
    bash tests/container/real-core-select-promotion-test.sh

# 严格验证 selection mirror 韧性 / cache.db 卫生 / TUN 事务诊断的容器合约
test-resilience-contract *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    bash tests/container/resilience-test.sh {{ARGS}}

# 聚合入口：runner 自检 + 全部离线容器测试 + user mode + 严格 systemd 合约
quick-test:
    bash tests/container/quick-test.sh

# 推送前完整检查（含真实 Core 验证链；需要 docker 与真实 Mihomo Core）
pre-push-check:
    just lint
    just test
    just quick-test
    just real-core-systemd-test
    just real-core-select-promotion-test

# 运行覆盖率分析（需要 cargo-tarpaulin）
coverage *THRESHOLD="23":
    scripts/coverage.sh {{THRESHOLD}}

# ━━━ 构建 ━━━

# Debug 构建
build:
    cargo build

# Release 构建
build-release:
    cargo build --release

# 交叉编译到指定 target
build-target TARGET:
    cargo build --target {{TARGET}} --release

# 使用 zigbuild 交叉编译（支持 macOS）
zigbuild-target TARGET:
    cargo zigbuild --target {{TARGET}} --release

# 安装到 ~/.cargo/bin/（release 构建 + 复制）
install:
    cargo build --release
    cp target/release/mihomo-cli ~/.cargo/bin/mihomo-cli
    echo "✅ mihomo-cli 已安装到 ~/.cargo/bin/"

# ━━━ 修复 ━━━

# 自动修复格式
fmt:
    cargo fmt

# 自动修复 clippy 警告
fix:
    cargo clippy --fix --allow-dirty --all-targets

# ━━━ 辅助 ━━━

# 列出所有可用命令
list:
    just --list

# 列出已安装的 Rust targets
targets:
    rustup target list --installed

# 检查哪些 targets 可以编译
check-targets:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "可编译的 targets:"
    for target in $(rustup target list --installed); do
        if cargo check --target "$target" --quiet 2>/dev/null; then
            echo "  ✓ $target"
        else
            echo "  ✗ $target（需要额外工具链）"
        fi
    done
