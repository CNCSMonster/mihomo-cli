# 快速开始

> 读者：使用者
> 回答：从零安装并跑通 system service 的最短路径
> 权威：本文为用户手册一部分

从零开始安装并运行 system service：

```bash
# 1. 安装 system service、Core、Geo 与授权基础设施；无订阅时生成 direct-only 配置并启动普通 Core/API，TUN 保持关闭
mihomo-cli install --system --yes

# 2. 导入本地 Clash/Mihomo YAML 配置
mihomo-cli config --import /path/to/config.yaml --activate --yes

# 3. system service 正在运行时，导入会受管 promotion 并等待 daemon/Core API；仅当输出为 pending/unknown/recovery，或需要应用 pending generation 时再执行：
# mihomo-cli restart --system

# 4. 查看当前代理组/节点
mihomo-cli list
```

如需全机透明代理，在基础服务/Core API readiness 已证明后再显式执行；命令成功仍须以 TUN runtime attestation 确认，不能替代真实目标数据面验证：

```bash
mihomo-cli tun on
```

如需彻底重装，可显式清理所有 mihomo-cli 实例后重复上述流程：

```bash
mihomo-cli uninstall --all --yes
```
