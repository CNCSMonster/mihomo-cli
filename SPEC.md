# mihomo-cli SPEC

> Mihomo CLI — cross-platform setup & control tool for Mihomo (Clash.Meta) proxy

---

## 1. Architecture

```
┌─────────────────────────────────────────────────────┐
│                   mihomo-cli                         │
│  (Rust, single binary, zero runtime dependencies)    │
├─────────────────────────────────────────────────────┤
│  main.rs      CLI entry + command routing (clap)     │
│  mihomo_api.rs  Unix socket REST client              │
│  config.rs     Subscription download + format convert │
│  installer.rs  Mihomo core binary download           │
│  service.rs    Service management + sudo dispatch     │
│  rules.rs      User-defined routing rule management   │
│  ui.rs         Interactive fuzzy-select (dialoguer)   │
│  utils.rs      File paths, service mode markers       │
├─────────────────────────────────────────────────────┤
│  ════════ Communication ════════                      │
│  ┌──────────┐   Unix socket    ┌─────────────┐       │
│  │mihomo-cli│ ←──────────────→ │  mihomo core  │      │
│  │          │  REST over HTTP  │  (daemon)     │      │
│  └──────────┘  /tmp/verge/     └─────────────┘       │
│                verge-mihomo.sock                      │
└─────────────────────────────────────────────────────┘
```

### 1.1 Communication Protocol

- **Transport**: Unix domain socket (`/tmp/verge/verge-mihomo.sock` on Unix, `\\.\pipe\mihomo` on Windows)
- **Protocol**: HTTP/1.0 over socket
- **Methods**: GET (query), PUT (select), PATCH (config), DELETE (connections)
- **Content-Type**: `application/json`

The socket path is configured in `config.yaml` via `external-controller-unix` / `external-controller-pipe`. mihomo-cli does NOT use HTTP port — it connects directly to the Unix socket.

### 1.2 Data Flow

```
mihomo-cli status
  → check binary exists at ~/.local/bin/mihomo
  → check config exists at ~/.config/mihomo/config.yaml
  → GET /configs via Unix socket
    ├─ success → parse response, fetch exit IP via api.ip.sb
    └─ failure → diagnose (see §3)

mihomo-cli select
  → GET /proxies → parse all Selector/URLTest/Fallback groups
  → flatten into (group, node, is_current) list (parse_selector_nodes)
  → dialoguer FuzzySelect over flat list → user picks
  → PUT /proxies/{group} {"name": "..."}

mihomo-cli proxy on|off
  → GET /configs → extract mixed-port
  → output shell export/unset commands → user pipes via eval

mihomo-cli restart
  → if service installed → restart_service() (see §4)
  → else → stop + start directly
```

---

## 2. Command Reference

### 2.1 Installation & Deployment

| Command | Description | Privilege |
|---------|-------------|-----------|
| `mihomo-cli install` | Full install: download mihomo core + config subscription + install service | May need sudo for service install |
| `mihomo-cli config` | Configure or update subscription URL | None |
| `mihomo-cli update` | Update mihomo core binary | None |
| `mihomo-cli uninstall` | Remove service (optionally all files) | May need sudo |
| `mihomo-cli version` | Print version info | None |

### 2.2 Service Control

| Command | Description | Privilege |
|---------|-------------|-----------|
| `mihomo-cli start [--user]` | Start mihomo service (default: root mode, prompts for sudo) | May need sudo |
| `mihomo-cli stop [--user]` | Stop mihomo service | May need sudo |
| `mihomo-cli restart [--user]` | Restart mihomo service | May need sudo |
| `mihomo-cli status` | Read-only diagnostic | **None** |

**Root vs User mode**:

| Mode | Command | Behavior |
|------|---------|----------|
| Root (default) | `mihomo-cli start` | Runs as root via sudo, TUN available on Linux |
| User | `mihomo-cli start --user` | Runs as current user, TUN unavailable on Linux |

Design decisions:
- **Root is default** — most users want TUN. Password prompt via `dialoguer::Password` + `sudo -S` (see §4).
- **`--user` flag** — consistent with `mihomo-cli install --user`. Pass it to start/stop/restart to operate on the user-level service.
- **Auto-detection** — if `--user` is not specified, reads `~/.config/mihomo/.service-mode` marker file to determine which mode was installed. Falls back to root if marker is missing.
- **TUN trade-off**: `--user` mode cannot enable TUN on Linux (requires `CAP_NET_ADMIN` / root). The command prints a warning if `tun on` is attempted in user mode.

### 2.3 Daily Operations

| Command | Description | Privilege |
|---------|-------------|-----------|
| `mihomo-cli select` | Fuzzy-search all proxy nodes across groups, select to switch | None (via socket) |
| `mihomo-cli list` | List all proxy groups and current node | None (via socket) |
| `mihomo-cli delay` | Test latency of nodes in a group | None (via socket) |
| `mihomo-cli proxy on/off` | Output shell export/unset commands for http_proxy/https_proxy | None (via socket, uses `eval`) |
| `mihomo-cli tun on/off` | Enable/disable TUN virtual NIC | None (via socket) |
| `mihomo-cli conn` | View active connections | None (via socket) |
| `mihomo-cli ip` | Probe exit IP via Direct + Mihomo proxy, diagnose TUN status | None (via socket for TUN state) |
| `mihomo-cli rule` | Manage user-defined routing rules (add/list/remove/clear/import/export/position) | None |
| `mihomo-cli dns` | Manage DNS routing policies (policy add/list/remove, status) | None |
| `mihomo-cli completions` | Generate shell completions | None |

#### `mihomo-cli ip` — Exit IP Diagnostic

Displays current environment state and probes three network paths:

```
=== Exit IP Report ===

  TUN:           disabled
  http_proxy:    not set
  https_proxy:   not set

  ISP               103.29.142.145  Hong Kong
  Now               103.29.142.145  Hong Kong
  Via Mihomo       54.116.44.255  South Korea
```

**Three probe lines:**

| Line | Meaning | How probed |
|------|---------|-----------|
| `ISP` | Pure ISP exit (no proxy, no TUN) | Direct probe when TUN off; cached from last TUN-off run when TUN on |
| `Now` | Current system route exit | `reqwest` without proxy — shows ISP when TUN off, proxy node when TUN on |
| `Via Mihomo` | Exit through mihomo proxy | `reqwest` via `http://127.0.0.1:{mixed-port}` |

**ISP cache:** When TUN is off, `probe_all_ips` writes the ISP result to `~/.config/mihomo/.isp_cache`. When TUN is on, reads from cache and labels `ISP (cached)`. No cache available → shows `(unreachable)`.

`--url` option:

`mihomo-cli ip --url <URL>` first makes a request to the target URL through each path, then checks the exit IP. This tests how mihomo's routing rules handle specific domains:

```
$ mihomo-cli ip --url https://github.com

=== Exit IP Report ===

  TUN:           disabled
  http_proxy:    not set
  https_proxy:   not set

  ISP               120.231.212.245  China
  Now               120.231.212.245  China
  Via Mihomo       1.2.3.4  United States
```

**Behavior:**

1. Read TUN state from mihomo API (`GET /configs`)
2. Read `http_proxy` / `https_proxy` environment variables
3. Probe ISP: direct when TUN off (with cache), cached when TUN on
4. Probe Now (system route): no proxy client
5. Probe Via Mihomo: via mihomo mixed-port
6. Diagnose with contextual messages (e.g. TUN enabled, proxy unreachable, LAN leak)

**Use cases:**
- Verify TUN is working (both paths show proxy IP)
- Check if shell is using proxy correctly
- Diagnose routing issues after config changes


#### `mihomo-cli rule` — User-Defined Routing Rules

Manage custom routing rules that are merged into the mihomo config at startup/reload. Rules are stored in a separate `rules.yaml` file and merged into `config.yaml` based on configurable insertion position.

**Subcommands:**

| Command | Description |
|---------|-------------|
| `mihomo-cli rule add <RULE>` | Add a rule (e.g. `DOMAIN-SUFFIX,example.com,DIRECT`) |
| `mihomo-cli rule add --position front\|back <RULE>` | Add with explicit position override |
| `mihomo-cli rule list` (alias: `ls`) | List all user-defined rules (1-based index) |
| `mihomo-cli rule remove <INDEX>` (alias: `rm`) | Remove rule by 1-based index |
| `mihomo-cli rule clear [--yes]` | Clear all rules (requires confirmation, `--yes` skips) |
| `mihomo-cli rule import <PATH>` | Import rules from a YAML file (replaces existing) |
| `mihomo-cli rule export <PATH>` | Export current rules to a YAML file |
| `mihomo-cli rule position [front\|back]` | Set or show default insertion position |

**How it works:**

1. User rules are stored in `~/.config/mihomo/rules.yaml` (YAML format, mihomo-compatible)
2. Insertion position is stored in `~/.config/mihomo/.rules-position` (default: `front`)
3. On every `start`/`restart`/`reload`, `merge_rules_to_config()` reads from `config.original.yaml` (the unmodified subscription config) and merges user rules into `config.yaml`
4. After each rule mutation, `PUT /configs` is called to hot-reload mihomo

**Merge strategy:**

- `config.original.yaml` is the clean source (saved by `install`/`config refresh`)
- `config.yaml` is the merged output (original + user rules)
- This prevents rule duplication on repeated merges

**Example:**

```bash
# Make company intranet go direct
mihomo-cli rule add DOMAIN-SUFFIX,company.com,DIRECT

# Make a specific domain use a proxy group
mihomo-cli rule add "DOMAIN-SUFFIX,google.com,节点选择"

# IP range direct
mihomo-cli rule add IP-CIDR,10.0.0.0/8,DIRECT

# Check merged result
mihomo-cli rule list
```



**Design Decisions:**

1. **Why `config.original.yaml`?**
   - Initial implementation merged rules into `config.yaml` directly
   - This caused rule duplication on repeated merges (each restart/reload would append user rules again)
   - Solution: Keep `config.original.yaml` as the clean merge source, always merge from original + user rules → `config.yaml`
   - `save_config()` now saves both files; `install` and `config refresh` create the initial `config.original.yaml`

2. **Merge Strategy**
   - `merge_rules_to_config()` is called in: `start_mihomo()`, `reload_configs()`, and after each rule mutation
   - Reads from `config.original.yaml` (if exists) or falls back to `config.yaml`
   - Merges user rules based on position config (front/back)
   - Writes result to `config.yaml`
   - Calls `PUT /configs` API for hot-reload

3. **Hot-Reload**
   - After each rule mutation (add/remove/clear/import), automatically calls mihomo API to reload
   - If API is unavailable, prints hint to run `mihomo-cli restart` manually
   - This ensures rules take effect immediately without manual intervention

4. **Non-Interactive Support**
   - `rule clear` normally requires confirmation via interactive prompt
   - Added `--yes` flag to skip confirmation for scripting/automation
   - Uses `interact_opt()` instead of `interact()` for safer TTY handling

### 2.4 Proxy Environment Variables

`mihomo-cli proxy` uses the standard `eval "$(cmd)"` pattern (same as `ssh-agent`, `direnv`):

```
Usage:
  eval "$(mihomo-cli proxy on)"    # exports http_proxy / https_proxy
  eval "$(mihomo-cli proxy off)"   # unsets all proxy variables

Design:
  - Port is read dynamically from mihomo API (/configs → mixed-port)
  - No hardcoded port — follows config changes automatically
  - Subprocess cannot modify parent shell env → eval pattern required
```

#### `mihomo-cli dns` — DNS Policy Management

Manage `nameserver-policy` entries that tell mihomo which DNS server to use for specific domains. Essential for internal company domains that can't be resolved by public DNS.

**Subcommands:**

| Command | Description |
|---------|-------------|
| `mihomo-cli dns policy add <MATCH> <TARGET>` | Add a DNS policy |
| `mihomo-cli dns policy list` (alias: `ls`) | List all DNS policies (1-based) |
| `mihomo-cli dns policy remove <INDEX\|MATCH>` (alias: `rm`) | Remove a policy |
| `mihomo-cli dns status` | Show current DNS configuration |

MATCH is a domain suffix (e.g. `ubtrobot.com`). TARGET is a DNS server IP address, or comma-separated list of IPs (e.g. `10.10.1.251,10.10.1.120`).

**Example:**

```bash
# Route company domain DNS queries to internal DNS servers
$ mihomo-cli dns policy add ubtrobot.com 10.10.1.251,10.10.1.120
  ✓ Policy added: ubtrobot.com → 10.10.1.251,10.10.1.120
  ✓ Config reloaded — DNS policy is now active

$ mihomo-cli dns policy list
  DNS policies:
  1. ubtrobot.com → system

$ mihomo-cli dns status
  DNS: enabled (fake-ip)
  Default nameservers: 114.114.114.114, 223.5.5.5, 119.29.29.29
  Fake-IP range: 28.0.0.1/8
  Listen: 127.0.0.1:1053
  Policies:
    1. ubtrobot.com → system
```

**Storage:** Policies stored in `~/.config/mihomo/dns-policy.yaml`, merged into `config.yaml` via `merge_user_config()`.

**Hot-Reload:** Uses mihomo API PATCH to apply immediately; also merged into config file for persistence across restarts.

### 2.5 Select UX (Flat Selector)

`mihomo-cli select` presents all selectable nodes as a flat list, merging groups:

```
  [节点选择] 韩国KR-HY2           ★ current
  [节点选择] 日本JP-HY2
  [节点选择] 新加坡SG-HY2
  [自动选择] 香港HK-HY2           ★ current
  [ChatGPT]  节点选择              ★ current
  [ChatGPT]  新加坡-优化-Gemini-GPT
  ...
```

Design decisions:
- **Flat over nested**: User searches by node name, not group. Type `jp` → filters to all Japan nodes regardless of group.
- **Current node marked ★**: Group prefix `[group]` provides context without requiring group awareness.
- **Fuzzy search**: `dialoguer::FuzzySelect` handles filtering.
- **--group flag preserved**: `mihomo-cli select --group 节点选择` delegates to existing group-scoped logic.

### 2.5.1 Why Flat?

- Nesting (group → node) forces the user to understand the group hierarchy.
- The group is an implementation detail of config — users just want to switch to "Japan" or "Singapore".
- Flat search eliminates the "node not found in default group" error that happens when subscription configs use different group names.

Implementation: `parse_selector_nodes()` in `mihomo_api.rs` extracts all Selector/URLTest/Fallback groups and flattens them into `Vec<(group, node, is_current)>`.

---

## 3. Status Diagnostic (Read-Only)

`mihomo-cli status` is a **pure diagnostic command**. It NEVER modifies system state. Logic:

```
status()
  ├─ binary missing     → "Fix: mihomo-cli install"
  ├─ config missing     → "Fix: mihomo-cli config"
  ├─ socket API works   → print: mode, TUN, port, node, exit IP
  └─ socket API fails ──→
       ├─ process alive + socket missing
       │   → "⚠ mihomo is running but API socket is missing"
       │   → "Fix: mihomo-cli restart"
       ├─ process alive + socket exists
       │   → "⚠ mihomo is running but unresponsive"
       │   → "Fix: mihomo-cli restart"
       └─ process dead
           → "❌ mihomo is NOT running"
           → "Fix: mihomo-cli restart"
```

### 3.1 Design Decision

**Q: Why doesn't `status` auto-repair?**

Because `status` should be safe for any user to run at any time, without side effects. Auto-repair was attempted (first versions) but introduced:
- Unexpected sudo password prompts during a "read-only" command
- Unclear causality (user ran `status`, something got restarted)
- Permission failures in non-TTY environments

**Separation of concerns**: `status` = diagnose, `restart` = repair.

---

## 4. Privilege Escalation (restart/systemctl)

When `mihomo-cli restart` needs to run `systemctl restart mihomo` (or equivalent), it uses a **smart dispatch** that avoids direct sudo interaction:

### 4.1 Smart Dispatch Flow

```
restart_service()
  │
  ├─ Case 1: Already root (UID 0)
  │   └─ systemctl restart mihomo          ← no password
  │
  ├─ Case 2: Sudo credentials cached
  │   └─ sudo -n systemctl restart mihomo  ← no password
  │      (checked via: sudo -n true)
  │
  └─ Case 3: Need password
      ├─ print: "The mihomo service runs as root."
      ├─ print: "Restarting it requires admin privileges."
      ├─ dialoguer::Password prompt        ← user types password
      └─ sudo -S systemctl restart mihomo  ← password piped via stdin
         (no TTY dependency)
```

### 4.2 Why not raw `sudo` subprocess?

| Approach | Problem |
|----------|---------|
| `sudo systemctl restart` (inherit TTY) | sudo may fail if `requiretty` is set in sudoers, or if TTY is not properly inherited by Rust subprocess |
| `pkill` → systemd auto-restart | Killing root process needs sudo; same TTY issue |
| **`sudo -S` with piped password** | Works regardless of TTY config; user only interacts with mihomo-cli's dialoguer prompt |

### 4.3 Safety

- 30-second timeout on all sudo commands
- If timeout expires, child process is killed
- Exit code of the command is always checked
- Fallback to clear error message with manual fix instruction

---

## 5. Socket Lifecycle

### 5.1 Normal Operation

```
1. mihomo starts (via systemd or direct)
2. mihomo creates Unix socket at /tmp/verge/verge-mihomo.sock
3. mihomo-cli connects to socket → API works
```

### 5.2 Socket Loss (The Problem)

The socket file can be deleted independently of the mihomo process:
- System tmpfiles cleanup (`systemd-tmpfiles-clean`)
- Manual deletion
- Reboot on systems that clear `/tmp`

**mihomo process is NOT affected** — proxy traffic continues. Only CLI management (select/list/delay/tun) breaks.

### 5.3 Recovery

**Only recovery method**: Restart mihomo (which recreates the socket).

- `mihomo-cli restart` → `sudo systemctl restart mihomo` → new socket at same path
- No data loss — mihomo stores state in config.yaml and cache.db, not in the socket

---

## 6. Installation Layout

```
~/.local/bin/
└── mihomo              # Mihomo core binary
└── mihomo-cli          # CLI tool

~/.config/mihomo/
├── config.yaml          # Clash YAML configuration (merged with user rules)
├── config.original.yaml # Clean subscription config (merge source, avoids duplication)
├── rules.yaml           # User-defined routing rules
├── .rules-position      # Rule insertion position: "front" (default) or "back"
├── start.sh             # Shell wrapper (macOS LaunchDaemon uses this)
├── mihomo.log           # Runtime log (if not using journalctl)
├── cache.db             # Proxy provider cache
├── geoip.metadb         # GeoIP database (pre-downloaded by mihomo-cli)
├── GeoSite.dat          # Domain classification database
├── geoip.metadb.tmp     # Partial download (resume-safe, cleaned on uninstall)
├── GeoSite.dat.tmp      # Partial download (resume-safe, cleaned on uninstall)
└── .service-mode        # Marker: "root" or "user"

/tmp/verge/
└── verge-mihomo.sock    # Unix domain socket

/etc/systemd/system/          # Linux (root mode)
└── mihomo.service
~/.config/systemd/user/       # Linux (user mode)
└── mihomo.service
/Library/LaunchDaemons/       # macOS
└── io.mihomo.plist
```

---

## 7. Subscription Processing

### 7.1 Format Detection

```
download_sub_smart(url)
  1. GET with User-Agent: "clash-verge/2.0.0"
     ├─ response contains "proxies:" / "mixed-port:" / "mode:"
     │   → Clash YAML, save directly ✅
     └─ non-YAML response
         → treat as raw subscription, convert
  2. If UA negotiation fails (HTTP error)
     → retry without UA header
```

### 7.2 Conversion (vmess/base64 → Clash YAML)

- Parses `vmess://` URLs (base64-decoded JSON)
- Parses `trojan://` URLs
- Detects base64-encoded subscription lists (tries padding variants)
- Generates full Clash YAML with:
  - Proxies
  - Proxy groups: `节点选择` (select), `自动选择` (url-test)
  - DNS (fake-ip mode)
  - TUN (enabled by default)
  - External controller (Unix socket)

### 7.3 Config Validation

After saving, runs `mihomo -t -d {config_dir}` to validate syntax.

---

## 8. Service Management

### 8.1 Platform Support

| Platform | Service Mechanism | Root Mode | User Mode |
|----------|------------------|-----------|-----------|
| Linux    | systemd           | `/etc/systemd/system/mihomo.service` | `~/.config/systemd/user/mihomo.service` |
| macOS    | LaunchDaemon      | `/Library/LaunchDaemons/io.mihomo.plist` | Not supported |
| Windows  | sc.exe            | Windows Service | Not supported |

### 8.2 Service Mode Tracking

A marker file `~/.config/mihomo/.service-mode` stores `"root"` or `"user"` to track which mode was installed. Default (if file missing): `"user"`.

### 8.3 Restart Behavior

- **Root mode** (`User=root`): systemd runs mihomo as root. TUN mode works. Restart needs `sudo`.
- **User mode** (`--user`): systemd runs mihomo as current user. TUN unavailable on Linux (needs root). Restart is passwordless.

---

## 9. Error Handling Philosophy

All errors follow this pattern:

```
1. Detect: what exactly is wrong
2. Report: human-readable message
3. Fix: actionable command the user can run
```

Example:
```
  ⚠  mihomo is running but the API socket is missing.
     The socket file was deleted (this doesn't affect proxy traffic)
     but CLI commands like select/list/delay won't work.

  Fix: mihomo-cli restart
```

Commands never silently fail. `-v` flag enables debug logging for troubleshooting.

---

## 10. Glossary

| Term | Meaning |
|------|---------|
| Mihomo | Clash.Meta core (proxy daemon) |
| Socket | Unix domain socket for CLI ↔ daemon communication |
| Subscription | Remote proxy list URL (vmess://, ss://, base64, or Clash YAML) |
| TUN | Virtual network interface for transparent proxy |
| systemd | Linux init system used for service management |
| LaunchDaemon | macOS service management mechanism |
| dialoguer | Rust library for interactive terminal prompts |
| Clash YAML | Configuration format used by mihomo/clash ecosystem |
| ISP | Internet Service Provider (互联网服务提供商) — 你的宽带/移动网络运营商分配的真实公网 IP |

---

## 11. Roadmap

### Done

| Feature | Description | Commit |
|---------|-------------|--------|
| proxy command | `eval "$(mihomo-cli proxy on/off)"` — dynamic port from API | `a5d848e` |
| parse_selector_nodes | Pure function + 5 unit tests | `44aa111` |
| start/stop/restart --user | User-level service control flag, consistent with install | `c50a3dd` |
| Flat select UX | `mihomo-cli select` — flat list across all groups, fuzzy search by node name | `51eaf20` |
| API readiness race | `wait_for_api_ready()` polls /configs after start/restart to prevent false "unresponsive" | `70b1809` |
| Geo pre-download | `ensure_geo_files()` downloads geoip.metadb/GeoSite.dat before mihomo starts — eliminates chicken-and-egg deadlock (mihomo needs proxy to reach GitHub, but isn't proxy yet). Resume support, mirror fallback, progress bar, corrupt detection. | `0d3029d`…`cfcc10a` |
| Rule management | `mihomo-cli rule add/list/remove/clear/import/export/position` — user-defined routing rules stored in `rules.yaml`, merged into `config.yaml` at start/reload with configurable insertion position (front/back). Uses `config.original.yaml` as clean merge source to prevent duplication. Design: rules stored in mihomo-compatible YAML format; `front` (default) gives user rules highest priority to override subscription; `back` as fallback; hot-reload via `PUT /configs` after each mutation; `config.original.yaml` saved by `install`/`config refresh` as clean merge source to prevent rule duplication on repeated merges. | — |

### Planned

| Feature | Notes |
|---------|-------|
| Config backup/restore | Snapshot config before risky operations |
| Desktop notification | Notify on node switch via D-Bus/notification center |
| Rule group support | Support rule-provider / rule-groups from external files |
| Rule position per-rule | Allow individual rules to override insertion position |

## 规则管理 V2：原子性配置更新

### 设计改进

**V1 问题**：
- 需要维护 `config.original.yaml` 和 `config.yaml` 两个文件
- 非原子性写入可能导致配置损坏
- 逻辑分散，难以维护

**V2 方案**：
- 使用标记法（`# === USER RULES START ===` / `# === USER RULES END ===`）
- 单一数据源：只维护 `config.yaml`
- 原子性写入：先写临时文件，再 rename
- 自动插入/替换 markers 之间的用户规则

### 实现细节

1. **标记插入**：首次添加规则时，在 `rules:` 部分插入 markers 和用户规则
2. **标记替换**：后续操作直接替换 markers 之间的内容
3. **原子写入**：`utils::atomic_write_file()` 确保写入的原子性
4. **位置控制**：支持 `front`（默认）和 `back` 两种插入位置

### 优势

- ✅ 无需维护 `config.original.yaml`
- ✅ 配置更新原子性，不会损坏
- ✅ 规则边界清晰，易于调试
- ✅ 支持手动编辑（markers 之间的内容）


## 12. Rule Management V2: Marker-Based Atomic Updates

### 设计动机

V1 实现使用 `config.original.yaml` 作为合并源，存在以下问题：
- 需要维护两个配置文件，增加复杂度
- 非原子性写入可能导致配置损坏
- 订阅更新时需要重新生成 original 文件

### V2 方案：标记法 + 原子写入

#### 核心思路

在 `config.yaml` 中使用特殊标记界定用户规则区域：

```yaml
rules:
# === USER RULES START ===
  - DOMAIN-SUFFIX,company.com,DIRECT
  - IP-CIDR,10.0.0.0/8,DIRECT
# === USER RULES END ===
  - DOMAIN-SUFFIX,google.com,Proxy
  - DOMAIN-SUFFIX,github.com,DIRECT
```

#### 工作流程

1. **添加规则**：
   - 如果标记不存在，在 `rules:` 后插入标记 + 规则
   - 如果标记已存在，替换标记之间的内容
   - 使用 `atomic_write_file()` 原子写入

2. **删除/清空规则**：
   - 更新 `rules.yaml`
   - 如果规则为空，移除整个标记块
   - 原子写入 `config.yaml`

3. **位置控制**：
   - `front`（默认）：标记插入到 `rules:` 后第一行
   - `back`：标记插入到规则列表末尾

#### 原子写入实现

```rust
pub fn atomic_write_file(path: &str, content: &str) -> Result<()> {
    let temp_path = format!("{}.tmp", path);
    std::fs::write(&temp_path, content)?;
    std::fs::rename(&temp_path, path)?;
    Ok(())
}
```

### 优势对比

| 特性 | V1 (config.original.yaml) | V2 (标记法) |
|------|---------------------------|-------------|
| 配置文件数量 | 2 个 | 1 个 |
| 写入原子性 | ❌ 非原子 | ✅ 原子 |
| 规则边界 | 隐式（需要对比） | ✅ 显式标记 |
| 手动编辑 | ❌ 困难 | ✅ 直观 |
| 订阅更新 | 需要重新生成 original | ✅ 自动处理 |
| 磁盘占用 | 较高 | ✅ 较低 |

### 关键实现

- `utils::atomic_write_file()`: 原子写入函数
- `config::merge_user_config()`: 基于标记的规则合并
- `config::merge_rules_marker_based()`: 标记插入/替换逻辑
- 清空规则时自动移除标记块

### 向后兼容

- 保留 `merge_rules_to_config()` 作为 legacy wrapper
- 现有 `rule` 命令接口不变
- `rules.yaml` 格式不变

---

## Core Design Principles

> 从实际 bug 中总结的 3 条关键设计原则，指导所有新功能的实现。

### 1. 前置条件检查：每个命令执行前校验状态

**原则**: 命令执行前检查前置条件是否满足，不假设 happy path。

**示例**:
- `install` 发现 config 已存在时，运行 `mihomo -t` 验证有效性，损坏则重新生成（而非直接跳过）
- `start` / `restart` 启动前检查 geo 文件完整性
- `status` 非 verbose 模式根据进程/socket/config 三种状态给出不同诊断

### 2. 关键操作原子化：要么全成功，要么可回滚

**原则**: 影响状态的操作用原子写入 + 中间状态标记，中断后不会留下不一致状态。

**示例**:
- Geo 文件下载: `.tmp` → `rename`（原子写入）；残留 `.tmp` 下次自动清空恢复
- 规则合并: 临时文件 → `rename` 覆盖 `config.yaml`；不修改原始订阅配置
- `uninstall --all`: 检查所有残留（binary、service、config、geo），全部清除

### 3. 失败路径也测试：不只有 happy path

**原则**: 同时为正常路径和故障路径编写测试和错误处理，确保系统在异常状态下也有合理行为。

**漏洞案例**:
- Ctrl+C 中断 install → 留下孤儿 config → `uninstall --all` 早期返回不处理 → 重新 install 复用损坏 config → mihomo 启动失败
- 新版 mihomo API 变更 → `delay` 命令 404 → 没有 fallback 路径或版本检测
- systemd 启动失败时日志文件不存在 → 错误提示指向不存在的文件，用户无法排查

---

## Architecture Decision Records (ADR)

### ADR-01: Unix socket over HTTP controller

**决策**: 使用 Unix socket (`/tmp/verge/verge-mihomo.sock`) 而非 HTTP TCP 端口作为 mihomo API 通信方式。

**理由**:
- 安全性：不暴露网络端口，仅本机访问
- 兼容 clash-verge-rev：使用相同 socket 路径，共享 mihomo 实例
- 权限隔离：socket 文件权限由 mihomo 进程控制

**影响**: 所有 API 调用走 Unix socket，不支持远程管理。

### ADR-02: 配置热重载 vs 重启

**决策**: 配置变更默认通过 `/configs` API 热重载，但 controller 变更必须重启。

**理由**: 
- 热重载无需中断服务
- controller 是启动参数，无法运行时变更

**影响**: `config --fix` 添加 controller 后提示用户 restart。

### ADR-03: 单二进制分发

**决策**: 纯 Rust 实现，不依赖外部工具（curl、jq、python3、fzf）。

**理由**:
- 零运行时依赖，跨平台安装体验一致
- Clap 提供完整的 CLI 体验（补全、帮助、模糊匹配）
- `dialoguer` 替代 fzf 实现交互式选择

### ADR-04: 预下载 Geo 数据

**决策**: mihomo-cli 在 install/start 时预下载 geoip.metadb 和 GeoSite.dat。

**理由**: 防止"鸡生蛋死锁"（mihomo 启动时需要 geo 文件，但代理还没启动无法下载）。

**影响**: 下载逻辑在 `installer.rs::ensure_geo_files()`，支持 GitHub + 镜像 fallback。

### ADR-05: 规则标记合并法

**决策**: 用户规则通过 YAML 标记 (`# === USER RULES START/END ===`) 合并到 config.yaml。

**理由**:
- 不修改原始订阅配置（config.yaml 是单一真相源）
- 标记可精确定位用户规则位置
- 支持 front/back 插入策略

### ADR-06: -v/--verbose 统一调试输出

**决策**: 使用 `--verbose` 全局标志 + `crate::log!()` 宏统一调试输出。

**理由**: 排查 API 失败、启动异常、配置问题时不需要修改代码。

**影响**: 所有 `crate::log!()` 调用仅在 `-v` 模式下输出到 stderr。

