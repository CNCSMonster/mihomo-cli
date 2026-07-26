# 并发变更操作 flock 保护方案

> 状态: 已确认
> 日期: 2026-07-22

## 1. 现状与问题

### 背景

mihomo-cli 是瘦客户端架构：每用户单个 mihomo core（由 LaunchAgent/systemd --user 管理），
CLI 进程可任意并发启动。**不限制 CLI 实例数**是正确设计（类比 docker/systemctl/brew），
但当前代码对文件变更操作**没有任何并发保护**。

### 操作分类

| 类型 | 命令 | 并发安全性 |
|------|------|-----------|
| A. 只读 / 纯 API | `status`、`delay`、`select`、`proxy on`、`rule test` | ✅ 安全。socket API 由 core 串行处理；只读文件无竞争 |
| B. 文件变更 | `config -u/--switch/--refresh`、`rule add/del`、`dns set`、`backup`、所有触发 merge 的操作 | ⚠️ 有竞争 |

### B 类竞争的具体风险

merge 流程是典型的 read-modify-write：

```
读订阅 YAML + rules.yaml + dns-policy.yaml + override.yaml
  → 内存合并 → 生成 config.yaml → 落盘 → 热重载
```

两个进程并发执行时：

| 场景 | 后果 |
|------|------|
| `rule add A` 与 `rule add B` 并发 | 双方基于同一旧版 rules.yaml 合并，后写者覆盖先写者，丢一条规则 |
| `config -u` 与 `dns set` 并发 | 一方生成的 config.yaml 被另一方整体覆盖，变更丢失 |
| 进程在写 YAML 中途被 kill | 订阅/config 文件半截内容，下次 merge 基于损坏文件继续 |
| `install` 与 `start` 并发 | 配置文件与服务状态不一致 |

### 必须支持的正常并发场景

1. 终端 A 开着 `select` TUI，终端 B 跑 `status` / `delay`（A 类，不可加锁阻塞）
2. skhd / Raycast / cron 脚本调 `tun off`、`proxy on`，与人工操作并发（A 类）
3. 手滑重复执行 `config -u`（B 类，应串行化而非报错）

## 2. 修复方向

### 2.1 决策：B 类操作加 flock 排他锁，A 类完全不加锁

- 锁文件：`{config_dir}/.mihomo-cli.lock`
- 机制：POSIX `flock(2)` 排他锁（macOS / Linux 均支持）
- 粒度：**进程内关键段**——只在执行文件变更的区间持锁，命令结束自动释放（fd 关闭即释放，进程崩溃不残留）
- 等待策略：**阻塞等待 + 超时**（建议 10 秒），超时给出友好提示而非直接失败

### 2.2 候选方案对比

| 方案 | 优点 | 缺点 | 结论 |
|------|------|------|------|
| `flock` on lockfile | 崩溃自动释放、跨 macOS/Linux、实现简单 | Windows 不支持 flock | **采用** |
| 创建锁目录（mkdir 原子性） | 跨平台含 Windows | 进程崩溃残留 stale lock，需 TTL/清理逻辑 | 否决（Windows 本就不涉及 unix socket 路径的同类竞争，后续可单独处理） |
| 限制单实例（全局互斥） | 最简单 | 阻塞 TUI+命令并行的正常场景，过度设计 | 否决 |
| 不做处理 | 零成本 | 低频但真实的文件损坏风险 | 否决 |

### 2.3 实现要点

```
src/lock.rs (新文件)
├── ConfigLock::acquire() -> Result<ConfigLockGuard>
│     - 打开/创建 {config_dir}/.mihomo-cli.lock
│     - flock(LOCK_EX)，带 10s 超时（LOCK_NB 轮询或 alarm 实现）
│     - 超时提示: "另一个 mihomo-cli 正在修改配置，请稍后重试"
└── Drop for ConfigLockGuard -> flock(LOCK_UN) + 关闭 fd
```

依赖选择：优先用 `libc::flock` 直接调（项目已有 libc 类依赖则零新增）；
否则引入 `fs2` crate。避免引入重量级依赖。

### 2.4 加锁点（B 类操作清单）

| 位置 | 操作 |
|------|------|
| `src/config.rs` `add_subscription_at` | 添加订阅 + merge |
| `src/config.rs` switch / refresh / remove 订阅 | 切换/刷新/删除 + merge |
| `src/rules.rs` rule add / del | 规则变更 + merge |
| `src/dns.rs` dns policy 变更 | DNS 变更 + merge |
| `src/backup.rs` backup / restore | 备份恢复 |
| `src/service.rs` `start_mihomo` 中的 merge 步骤 | 启动前重新生成 config |
| `src/main.rs` `merge_subscription_change_checked` 调用处 | 统一 merge 入口 |

**推荐做法**：在 merge 的统一入口（`merge_user_config_checked_at` / `merge_subscription_change_checked`）
内部 acquire 锁，而不是在每个命令入口加——锁的范围自然覆盖"读输入文件 → 写 config.yaml → 热重载"整个关键段，
且新增变更操作自动获得保护。

注意：`start_mihomo` 也走 merge，与 `config -u` 并发时同样需要互斥。

### 2.5 不加锁的部分（明确排除）

- `select` / `status` / `delay` / `proxy` / `rule test`：A 类，保持无锁
- socket API 调用：core 侧串行处理
- `delay-cache.json` 写入：独立小文件，损坏代价低（重写即可），不值得持全局锁；如需完善可用"写临时文件 + rename"原子替换

### 2.6 原子写（顺带改进）

config.yaml / rules.yaml / subscriptions.yaml 的落盘统一改为：

```
写 {file}.tmp → fsync → rename 到目标文件
```

rename 在同文件系统内是原子的，消除"写一半被 kill 留下半截 YAML"的损坏窗口。
这与 flock 互补：flock 防进程间竞争，原子写防单进程中途崩溃。

## 3. 兼容性

- 锁文件新增于 `{config_dir}/.mihomo-cli.lock`，不影响现有文件布局
- 旧版本 CLI 不加锁，与新版本混用时新版本仍受保护（旧版本不参与锁协议，风险与现状相同）
- Windows：flock 不可用，首版可 `#[cfg(unix)]` 实现，Windows 侧退化为无锁（现状），后续按需补 named mutex

## 4. 验证

- 单元测试：两个线程/进程同时 acquire，后者阻塞至前者释放；超时路径返回友好错误
- 集成测试：并发跑 `rule add` × 2，断言两条规则都存在于 rules.yaml
- 回归：TUI `select` 打开期间执行 `status` / `delay` 不受锁影响
