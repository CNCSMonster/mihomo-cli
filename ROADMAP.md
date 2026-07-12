# mihomo-cli ROADMAP

> 功能规划 + 已知问题 + Bug 追踪

---

## 🐛 Bug Tracker

### BUG-01: GeoSite.dat 重复下载 ✅ 已修复
- **发现**: start 失败后 restart 仍然重复下载依赖，即使先 install 且 ret 0
- **症状**: `GeoSite.dat exists but first byte is invalid, re-downloading`
- **根因**: 下载流结束后未校验完整性（无 Content-Length 比对），截断/损坏文件被当作成功
- **修复**: 四层校验 — ① Content-Length 大小比对 ② 首字节检查（已有） ③ `mihomo -t` 最终验证 ④ `sync_all()` 保证落盘

### BUG-02: GeoIP MMDB 鸡生蛋死锁 ✅ 已修复
- **发现**: mihomo 启动时需要下载 geoip.metadb，但代理还没启动 → 死循环
- **现象**: socket 文件存在但无人监听，mihomo 进程卡在 MMDB 下载
- **修复**: mihomo-cli 侧预下载 geo 文件（install [4/4]、config --refresh、start/restart 前）
- **提交**: installer.rs — `ensure_geo_files()`, `download_geo_with_fallback()`

### BUG-03: delay 命令 404 ✅ 已修复
- **发现**: 新版 mihomo 将 group delay API 从 `/proxies/{name}/delay` 迁移到 `/group/{name}/delay`
- **修复**: 更新端点路径 + 修复 query 参数双重编码问题
- **提交**: bcf5a4a

### BUG-04: start 失败后日志文件缺失 ✅ 已修复
- **发现**: systemd service 启动失败时日志文件未创建，错误提示无法排查
- **修复**: 日志缺失时自动运行 `mihomo -t` 诊断配置语法，直接展示错误
- **提交**: 8a7b249

---

## ✅ 已完成

### v0.1.0 核心功能
- [x] 安装部署：mihomo 核心下载 + 配置生成 + 系统服务
- [x] 订阅管理：vmess:// / base64 / Clash YAML 自动格式转换
- [x] 节点选择：fzf 交互式模糊搜索 (`select`)
- [x] TUN 模式：开关透明代理 (`tun on/off`)
- [x] 延迟测试：组内节点延迟 (`delay`)
- [x] 出口 IP 探测：直连 vs 代理路径诊断 (`ip`)
- [x] 连接管理：查看/关闭活跃连接 (`conn`)
- [x] Shell 代理：输出代理环境变量 (`proxy on/off`)

### Rule Management (2026-07-09)
- [x] 用户自定义路由规则管理 (`rule add/list/remove/clear/import/export/position`)
- [x] 规则存储在 `~/.config/mihomo/rules.yaml`
- [x] 支持配置插入位置 (front/back)
- [x] 自动合并到 config.yaml 并热重载

### 可排查性优化 (2026-07-10)
- [x] 统一 `--verbose` 诊断输出
- [x] service install --dry-run
- [x] install --skip-geo 跳过数据文件
- [x] delay/install 自动修复配置 (Unix socket controller)
- [x] 错误信息指向日志文件
- [x] 启动时 socket 就绪检查 + 自动修复

### 稳定性与体验优化 (2026-07-13)
- [x] macOS user mode (LaunchAgent) 支持 — 无需 sudo 的用户级服务
- [x] install 交互选择 root/user 模式
- [x] TLS 证书修复 — 使用系统根证书 (rustls-native-certs) 解决订阅下载失败
- [x] 文档体系建立 — README, USAGE, SPEC, ROADMAP, CONTEXT, CONTRIBUTING, CHANGELOG
- [x] 设计原则确立 — 前置条件检查、原子操作、失败路径测试
- [x] start_mihomo 启动前 config 预检 (mihomo -t)
- [x] 失败路径测试覆盖 (11 个新测试)

---

## 📋 Backlog

### High Priority
- [ ] 规则格式验证 — 添加规则时检查 TYPE,PARAMETER,POLICY 格式
- [ ] 规则去重 — 可选自动去除重复规则
- [ ] 改进错误提示 — 更友好的错误信息和恢复建议

### Medium Priority
- [ ] 规则排序 — 支持 `rule move` 命令调整规则顺序
- [ ] 规则组支持 — rule-provider / rule-groups
- [ ] 每条规则独立位置配置

### Low Priority
- [ ] Config backup/restore — 危险操作前自动备份配置
- [ ] Desktop notification — 节点切换时通知
- [ ] 规则导入时合并而非替换
- [ ] 规则搜索/过滤 — `rule search <keyword>`
