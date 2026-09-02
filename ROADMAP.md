# mihomo-cli ROADMAP

> 顶层方向规划。实现细节见 `SPEC.md`、`docs/` 设计文档；具体任务用 GitHub Issues 追踪。

---

## 项目定位（稳定）

跨平台 **Mihomo (Clash.Meta) 代理管理 CLI**：安装部署 + 日常控制合一，单二进制零依赖。

- **平台**：macOS / Linux 一等公民，Windows 二等公民（CI 验证）
- **形态**：命令行优先，TUI 只是人类友好外壳
- **服务对象**：人类终端用户 + AI Agent / 脚本

## 方向支柱（稳定，不随迭代变动）

| 支柱 | 说明 |
|------|------|
| **跨平台一致** | 三平台核心命令行为一致，路径/服务机制按平台原生实现 |
| **单二进制零依赖** | 不依赖 Python/Node/系统包，`cargo install` 即用 |
| **安装 + 控制合一** | `install → config import/add → restart → status` 一条龙，无第二工具；TUN 仍需显式 `tun on/off` |
| **AI-Ready** | 机器可读（`--json`）、确定性输出、幂等语义、非交互（`--yes`） |
| **单内核专注** | 只管理 mihomo 内核，不引入 sing-box 等多内核 |
| **安全加固** | TUN root 门控、配置隔离、token 认证、符号链接防护、日志脱敏 |

## 已完成里程碑

| 里程碑 | 内容 | 版本 |
|--------|------|------|
| **核心功能** | install/restart/stop/TUN/订阅/节点选择/延迟测试/显式 exit-ip probe | v0.1.0 |
| **规则管理** | `rule add/list/remove/clear/import/export/position` | 2026-07-09 |
| **可排查性** | 统一诊断输出、install 预检、错误定位、日志修复 | 2026-07-10 |
| **稳定性** | macOS user mode、TLS 证书修复、原子写操作、失败路径测试 | 2026-07-13 |
| **写操作安全** | checked merge + 回滚、备份恢复、DNS 模板、系统代理 | 2026-07-16~17 |
| **Instance Model** | 互斥双模式（User/System）自动检测、daemon IPC、路径矩阵；跨平台行为按各自证据等级验证 | v0.3.0 |
| **安全加固** | L1-L7 设计覆盖 TUN root / 配置隔离 / token / socket / daemon 非 root / 符号链接 / 日志脱敏；完整旅程和真实 data plane 仍按证据矩阵分别报告 | v0.4.0 |
| **Windows 服务** | SCM 协议、named pipe SDDL、TokenElevation、autostart 设计；Windows service/TUN/full journey 不以单次 CI 或局部 contract 概括 | v0.5.0 |

## 方向里程碑

### M1: AI-Ready CLI（进行中）

**目标**：让 AI Agent / 脚本能稳定、安全地编排 mihomo-cli。

- 机器可读输出：统一 JSON envelope（`ok/command/data/warnings/error/meta`）
- 确定性错误码与退出码契约
- 幂等命令语义：重复执行结果可预期
- 非交互契约：`--yes` / `--dry-run` / `--json` 组合，不阻塞等待 TUI
- Secret 脱敏：订阅 token / mihomo secret / 节点凭据默认隐藏
- Agent Skill 封装：供 Qwen / Claude / Codex 等上层 AI 调用

**现状**：`status/version/config --validate --json` 已有对应设计/实现边界；config CLI 子命令化设计见 `docs/SPEC-config-cli.md`。

**验收**：核心命令（status/config/proxy/list/select/delay/doctor）全部支持 `--json`、错误码、幂等、脱敏；README/USAGE 有 AI 快速开始示例。

### M2: 订阅管理增强（规划中）

**目标**：灵活、可诊断的订阅源管理。

- `config` CLI 子命令化：`add/remove/switch/list/info/refresh/probe/fetch/import`
- 订阅 probe 结构化：每个 UA candidate 的 HTTP 状态、格式识别、节点数、可用性
- 订阅 URL 默认脱敏
- 订阅漂移检测：刷新/切换后提示用户规则引用失效，不自动修改
- 多订阅一致性：refresh/switch/import 后 reload 行为统一，缓存与生效配置一致

**现状**：离线 fetch 旅程（J001）已实现；probe 结构化与脱敏规划中。

**验收**：多订阅增删切换无残留、漂移有 warning、token 不泄露。

### M3: 平台体验打磨（部分完成）

**目标**：各平台原生运维体验。

- **Windows**：服务日志落盘（✅ 完成）、失败自动恢复（规划）、token 双校验（规划）
- **macOS**：launchd 生命周期完善（✅ 完成）、真机回归（进行中）
- **Linux**：systemd 生命周期完善（✅ 完成）、真机回归（进行中）
- **跨平台**：CI 矩阵覆盖三平台契约测试（规划）

**现状**：Windows SCM/权限/服务实现仍按对应 contract、真实 Core 和平台 E2E 证据分别报告；macOS/Linux 的服务生命周期也按平台和旅程证据分别报告，不能用设计存在或单次 CI 结果概括完整支持。

**验收**：三平台核心命令契约测试全绿；Windows 服务日志可查、崩溃可恢复。

### M4: 安全加固（基本完成）

**目标**：威胁模型驱动的纵深防御。

- L1-L7 已纳入当前安全设计与实现边界：TUN root 门控、配置隔离、token 认证、socket 审计、daemon 非 root、符号链接防护、日志脱敏；各平台完整旅程和真实 data plane 仍按 `SPEC.md §0.4` 分层报告
- 待评估/持续验证：多用户 daemon 访问控制、部分安装事务 crash points、跨平台真实 Core/TUN evidence

**现状**：L1-L7 已形成统一安全设计和对应实现/测试边界；具体平台、真实 Core、TUN 和 data plane 证据按 `docs/SECURITY.md` 与 `SPEC.md §0.4` 分层报告。

**验收**：安全边界明确（`docs/SECURITY.md` §5），已知限制有缓解计划。

## 非目标（明确不做）

| 方向 | 原因 |
|------|------|
| 多 Profile / 多套工作区切换 | 当前设计收敛为单一配置事实来源；Profile 是远期方向 |
| GUI / 托盘 / 桌面产品化 | 与 CLI 定位冲突，桌面场景由 clash-verge-rev 承担 |
| sing-box 等多内核支持 | 单内核专注决策（2026-08-02） |
| 大型 TUI 重写 | 现有 dashboard/select TUI 只做薄封装 |
| `doctor --fix` / `repair` | 诊断阶段只给建议，不做自动修复破坏性动作 |
| 内置 AI 能力 | AI 原生化方向是"被 AI 使用"，不是"内置 AI" |

## 已知限制

- **多用户 daemon 访问控制**：授权表、token 和 IPC peer 校验的完整跨平台行为仍需按平台证据验证
- **跨平台 service 证据不对称**：Windows/macOS/Linux 的 service、Core/API、TUN 和真实 data plane 不能用同一层级的测试结果互相替代
- **真实 TUN/data-plane fixture**：缺少同架构真实 Core、privileged netns 或外部 probe 时，只能报告 `Contract-tested`/`Planned`，不能报告 `Full-journey-tested`
- **部分安装与 recovery**：journal/manifest/残留身份无法证明时必须 fail-closed 并返回 `RecoveryRequired`；实现和测试仍需覆盖所有 crash points
- **运行态可观察性**：Core/API 不可达时 `runtime_tun`、live ports、rule mode 必须为 `unknown`，不能由磁盘 intent 或 daemon 缓存推断

## 待办入口

具体任务追踪（不再放 ROADMAP）：

- GitHub Issues（建议）：M1-M4 的拆分任务
- `docs/SPEC-*.md`：功能设计文档
