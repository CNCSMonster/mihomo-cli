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
│  ui.rs         Interactive selector (crossterm TUI)   │
│  utils.rs      File paths and runtime directories         │
├─────────────────────────────────────────────────────┤
│  ════════ Communication ════════                      │
│  ┌──────────┐   Unix socket    ┌─────────────┐       │
│  │mihomo-cli│ ←──────────────→ │  mihomo core  │      │
│  │          │  REST over HTTP  │  (daemon)     │      │
│  └──────────┘                  └─────────────┘       │
│  Linux: $XDG_RUNTIME_DIR/mihomo/                      │
│  macOS: /tmp/mihomo-$UID/                             │
└─────────────────────────────────────────────────────┘
```

### 1.1 Communication Protocol

- **Transport**: Unix domain socket (platform-specific)
  - Linux: `$XDG_RUNTIME_DIR/mihomo/mihomo.sock` (typically `/run/user/$UID/mihomo/mihomo.sock`)
  - macOS: `/tmp/mihomo-$UID/mihomo.sock`
  - Windows user: `\\.\pipe\mihomo-$USERNAME`; Windows system core: `\\.\pipe\mihomo-core`; Windows system service IPC: `\\.\pipe\mihomo-service`
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
  → crossterm TUI: j/k navigate, / filter, Enter select, Esc cancel
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
| `mihomo-cli config` | Interactive subscription TUI (keyboard shortcuts), or `--add/--remove/--list/--switch/--validate/--dry-run/--import/--info/--probe/--set-ua/--refresh/--refresh-all` | None |
| `mihomo-cli update` | Update mihomo core binary | None |
| `mihomo-cli uninstall` | Remove service (optionally all files) | May need sudo |
| `mihomo-cli version` | Print version info | None |

### 2.2 Service Control

| Command | Description | Privilege |
|---------|-------------|-----------|
| `mihomo-cli start` | Start mihomo service (auto-detects mode) | None (IPC to service) |
| `mihomo-cli stop` | Stop mihomo service | None (IPC to service) |
| `mihomo-cli restart` | Restart mihomo service | None (IPC to service) |
| `mihomo-cli status` | Read-only diagnostic | **None** |

**Mutually Exclusive Modes** (see [SPEC-architecture-v3](docs/design/SPEC-architecture-v3.md)):

| Mode | Install Command | Behavior |
|------|----------------|----------|
| System Service | `mihomo-cli install --system` | Runs as root, TUN available, one-time password |
| Per-user Service | `mihomo-cli install --user` | Runs as current user, TUN unavailable |

Design decisions:
- **Auto-detection** — daily commands first inspect runtime state (daemon IPC, core API socket, service-manager active state), then fall back to installed service artifacts or the default per-user mode.
- **Interactive install** — `mihomo-cli install` (no flags) prompts user to choose system or per-user mode.
- **Explicit system override** — lifecycle/API/status commands auto-detect by default; use `--system` only when explicitly targeting the system service. `--user` remains available only on install/uninstall as the public mode-selection flag.
- **TUN trade-off**: Per-user mode cannot enable TUN. The command prints a clear error if `tun on` is attempted in per-user mode.
- **Conflict detection** — if system and user runtimes are both active, commands fail fast and tell the user to stop one mode. Installed-service conflicts also fail for mutating operations.

### 2.3 Daily Operations

| Command | Description | Privilege |
|---------|-------------|-----------|
| `mihomo-cli select` | crossterm TUI: j/k navigate, / filter, Enter select, Esc cancel | None (via socket) |
| `mihomo-cli list` | List all proxy groups and current node | None (via socket) |
| `mihomo-cli delay` | Test latency of nodes in a group | None (via socket) |
| `mihomo-cli proxy on/off` | Output shell export/unset commands for http_proxy/https_proxy | None (via socket, uses `eval`) |
| `mihomo-cli tun on/off` | Enable/disable TUN virtual NIC (`--stack gvisor`, `--dns-hijack`) | None (via socket) |
| `mihomo-cli conn` | View active connections (`--flush` to close all) | None (via socket) |
| `mihomo-cli rule` | Manage user-defined routing rules (add/list/remove/clear/import/export/position) | None |
| `mihomo-cli dns` | Manage DNS routing policies (policy add/list/remove, status, template list/apply) | None |
| `mihomo-cli system-proxy on/off` | Set/unset OS system proxy (macOS: networksetup, Linux GNOME: gsettings) | May need sudo |
| `mihomo-cli logs` | View mihomo log file (`--level`, `--tail`) | None |
| `mihomo-cli backup` | Backup all config files to a directory | None |
| `mihomo-cli restore` | Restore config files from a backup directory | None |

#### Exit IP and target routing boundaries

`mihomo-cli` intentionally does **not** provide a standalone `ip` command.

Current supported responsibilities:

| User question | Command | Semantics |
|---------------|---------|-----------|
| Is mihomo running? Is TUN enabled? What is the current proxy probe result? | `mihomo-cli status` | Fast runtime overview. The proxy probe is measured through the current Mihomo proxy path with short timeout and hedged fallback over IP echo services. |
| Which policy will a domain match? | `mihomo-cli rule test <host>` | Static rule evaluation against the generated config and user rules. |
| What connections are currently active? | `mihomo-cli conn` | Runtime connection list from Mihomo external-controller. |

Why no `ip --url`:

- Mihomo is rule-based. A target URL and an IP echo URL are different destinations and can match different rules.
- Probing `https://api.ip.sb` through the normal proxy path reports the route of the IP echo service, not necessarily the route of the user's target URL.
- Mihomo's public external-controller delay/healthcheck API can ask a specified proxy to access a URL, but it returns delay/health status, not the target response body.
- Temporarily switching selector groups or launching isolated Mihomo cores would introduce side effects and significant complexity.

Deferred future feature:

Per-node or per-target egress IP probing may be added only when Mihomo or a compatible core exposes a stable control-plane API that can fetch a URL through a specified proxy and return the response body, or a dedicated safe egress-IP probe API.


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
3. On every `start`/`restart`/`reload`, `merge_user_config()` validates YAML, injects the resolved runtime controller endpoint, and merges user rules/DNS policies.
4. After each rule mutation, `PUT /configs` is called to hot-reload mihomo when the resolved runtime is reachable.

**Merge strategy (V3 - serde_yaml validated, marker based):**

- `config.yaml` is the single source of truth
- User rules are delimited by `# === USER RULES START ===` / `# === USER RULES END ===` markers
- YAML syntax is validated with `serde_yaml` before/after edits; invalid YAML fails explicitly
- Runtime controller fields are regenerated from the resolved instance endpoint, preventing stale system/user endpoint leakage
- Writes `config.yaml` atomically (write .tmp then rename)
- No `config.original.yaml` needed — `config.yaml` is the single source of truth
- Avoids native YAML parser dependencies so Windows GNU/MSVC target checks do not require tree-sitter C builds

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

1. **Why serde_yaml-validated marker editing (V3b)?**
   - V2 implementation used unchecked string operations to insert markers, causing YAML indentation bugs
   - The interim tree-sitter approach improved structure awareness but added native C parser dependencies that complicated Windows target validation
   - Current implementation validates input/output with serde_yaml, keeps marker-based user rule blocks, and updates DNS policy through serde_yaml mappings
   - Runtime controller fields are regenerated from the resolved instance endpoint on every merge/fix path
   - Fails explicitly on invalid YAML instead of silent degradation (see ADR-11)

2. **Merge Strategy**
   - `merge_user_config()` is called in: `start_mihomo()`, `reload_configs()`, and after each rule mutation
   - Parses config.yaml with serde_yaml, inserts/replaces user rules in the marker section, and updates DNS policy as structured YAML
   - Validates result as legal YAML before writing
   - Writes result atomically (write .tmp then rename)
   - Calls `PUT /configs` API for hot-reload when runtime is reachable

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

### 2.5 Select UX (Flat Selector with crossterm TUI)

`mihomo-cli select` presents all selectable nodes as a flat list, merging groups:

```
  Select node (j/k navigate, / filter, Enter select, Esc cancel)
  120/120  (press / to filter)

  ▶ [节点选择] 韩国KR-HY2           ★
    [节点选择] 日本JP-HY2
    [节点选择] 新加坡SG-HY2
    [自动选择] 香港HK-HY2           ★
    [ChatGPT]  节点选择              ★
    [ChatGPT]  新加坡-优化-Gemini-GPT
    ...

  j/k: navigate  /: filter  Enter: select  Esc: cancel
```

**Keyboard shortcuts:**
- `j` / `↓`: Move cursor down
- `k` / `↑`: Move cursor up
- `g`: Jump to top
- `G`: Jump to bottom
- `/`: Enter filter mode (type to fuzzy-search)
- `Enter`: Select current node
- `Esc`: Cancel

**Filter mode:**
- Press `/` to enter filter mode
- Type to filter nodes (case-insensitive substring match)
- `Backspace`: Delete last character
- `Esc`: Exit filter mode (clears filter)

Design decisions:
- **Flat over nested**: User searches by node name, not group. Type `jp` → filters to all Japan nodes regardless of group.
- **Current node marked ★**: Group prefix `[group]` provides context without requiring group awareness.
- **crossterm TUI**: Replaced `dialoguer::FuzzySelect` with custom crossterm-based TUI for better keyboard navigation (j/k vim-style).
- **--group flag preserved**: `mihomo-cli select --group 节点选择` delegates to existing group-scoped logic.

### 2.5.1 Why Flat?

- Nesting (group → node) forces the user to understand the group hierarchy.
- The group is an implementation detail of config — users just want to switch to "Japan" or "Singapore".
- Flat search eliminates the "node not found in default group" error that happens when subscription configs use different group names.

Implementation: `parse_selector_nodes()` in `mihomo_api.rs` extracts all Selector/URLTest/Fallback groups and flattens them into `Vec<(group, node, is_current)>`.

### 2.6 Config TUI (Subscription Management)

`mihomo-cli config` (without subcommand flags) launches an interactive crossterm-based TUI for managing subscriptions:

```
  Subscription sources
  ↑↓ navigate · Enter switch · r refresh · R refresh all · a add · d delete · Esc/q quit

  ▶ sub-e5918a16 ★ (active)
    https://msub.example.com/api/v1/client/subscribe?token=...
    sub-abcdef12
    https://other.example.com/sub

  j/k: navigate  /: filter  Enter: select  Esc: cancel
```

**Keyboard shortcuts (real key capture via crossterm raw mode):**
- `↑` / `k`: Move cursor up
- `↓` / `j`: Move cursor down
- `Enter`: Switch to selected subscription
- `r`: Refresh active subscription (re-download from URL)
- `R`: Refresh all subscriptions
- `a`: Add new subscription source (prompts for URL)
- `d`: Delete subscription under cursor
- `Esc` / `q`: Quit TUI

**Design decisions:**
- **crossterm raw mode**: Unlike `dialoguer::Select` (which only supports ↑↓/Enter/Esc), crossterm captures individual key presses, enabling true keyboard shortcuts.
- **Non-interactive fallback**: When stdin is not a terminal (e.g. piped input), the TUI degrades gracefully to flag-based commands (`--add`, `--list`, etc.).

### 2.7 Multi-Subscription Management

`mihomo-cli config` supports multiple subscription sources via flags:

| Flag | Description |
|------|-------------|
| `--add <URL>` | Add a new subscription source |
| `--remove <ID>` | Remove a subscription by ID |
| `--list` | List all subscription sources |
| `--switch <ID>` | Switch active subscription |
| `--info [ID]` | Show subscription info (node count, update time, expiry) |
| `--refresh` | Refresh active subscription from URL |
| `--refresh-all` | Refresh all subscriptions from URLs |
| `--probe <URL>` | Probe a URL with bounded UA candidates (auto/ClashMeta/Clash) without writing |
| `--set-ua <ID> <UA\|auto>` | Set subscription User-Agent mode (fixed or auto-probe) |
| `--user-agent <UA>` | Use a fixed User-Agent for add/refresh URL fetching |
| `--validate` | Validate current config.yaml with YAML parser and `mihomo -t` |
| `--dry-run` | Preview config operation without writing or restarting |
| `--import <FILE>` | Import config from a local file (auto-detect base64/vmess/Clash YAML) |
| `--yes` | Assume yes for prompts (e.g. activate imported subscription) |

**Storage:**
- `~/.config/mihomo/subscriptions.yaml` — subscription metadata (id, url, updated, ua_mode)
- `~/.config/mihomo/subscriptions/<id>.yaml` — downloaded subscription content
- `~/.config/mihomo/subscriptions/active` — active subscription ID

**UA Probing:** Some subscription servers return different formats based on User-Agent. `--probe` tests the URL with multiple UAs (auto, ClashMeta, Clash) and picks the best response. `--set-ua <id> auto` enables automatic probing on each refresh.

### 2.8 Backup & Restore

`mihomo-cli backup` / `mihomo-cli restore` snapshot and restore all config files:

```bash
# Backup all config files
mihomo-cli backup                    # → ~/.config/mihomo/backups/<timestamp>/
mihomo-cli backup /path/to/dir       # → custom directory

# Restore from backup
mihomo-cli restore /path/to/backup
mihomo-cli restore                   # → interactive selection from ~/.config/mihomo/backups/
```

**Files backed up:**
- `config.yaml`, `rules.yaml`, `dns-policy.yaml`, `override.yaml`
- `subscriptions.yaml`, `subscriptions/` directory
- `.rules-position`

**Design:** Backup is a simple file copy. Restore replaces files and restarts mihomo. No versioning/diff — users manage backup directories themselves.

### 2.9 System Proxy

`mihomo-cli system-proxy on/off` sets/unsets OS-level system proxy:

| Platform | Mechanism |
|----------|-----------|
| macOS | `networksetup -setwebproxy` / `-setsecurewebproxy` / `-setsocksfirewallproxy` |
| Linux GNOME | `gsettings set org.gnome.system.proxy` |
| Linux other | Prints manual instructions (no universal API) |

```bash
mihomo-cli system-proxy on     # Set system proxy to 127.0.0.1:<mixed-port>
mihomo-cli system-proxy off    # Unset system proxy
```

### 2.10 Logs Viewer

`mihomo-cli logs` displays the mihomo log file:

```bash
mihomo-cli logs                # Tail last 50 lines
mihomo-cli logs --tail 100     # Tail last 100 lines
mihomo-cli logs --level warn   # Filter by log level (debug/info/warn/error)
mihomo-cli logs -f             # Follow mode (like tail -f)
```

### 2.11 Override.yaml

`~/.config/mihomo/override.yaml` allows arbitrary field overrides that are deep-merged into `config.yaml` after subscription content:

```yaml
# override.yaml example
proxy-groups:
  - name: "Custom Group"
    type: select
    proxies: ["DIRECT"]
dns:
  enable: true
  enhanced-mode: fake-ip
```

**Merge order:** subscription content → `override.yaml` → user rules → DNS policies → controller injection.

**Use case:** Add custom proxy-groups, override DNS settings, or inject rules without modifying the subscription file.

### 2.12 DNS Templates

`mihomo-cli dns template` provides common DNS policy templates:

```bash
mihomo-cli dns template list              # List available templates
mihomo-cli dns template apply <NAME>      # Apply a template
```

**Built-in templates:**
- `company` — Route company intranet domains to internal DNS
- `anti-ad` — Block ad domains via DNS rejection
- `doh` — Use DNS-over-HTTPS for all queries

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
2. mihomo creates Unix socket:
   - Linux: $XDG_RUNTIME_DIR/mihomo/mihomo.sock
   - macOS: /tmp/mihomo-$UID/mihomo.sock
3. mihomo-cli connects to socket → API works
```

### 5.2 Socket Loss (The Problem)

The socket file can be deleted independently of the mihomo process:
- System runtime directory cleanup on logout
- Manual deletion
- Reboot

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
├── config.yaml          # Clash YAML configuration (merged with user rules via markers)
├── rules.yaml           # User-defined routing rules
├── dns-policy.yaml      # DNS routing policies (nameserver-policy)
├── .rules-position      # Rule insertion position: "front" (default) or "back"
├── geoip.metadb         # GeoIP database (pre-downloaded by mihomo-cli)
├── GeoSite.dat          # Domain classification database
├── geoip.metadb.tmp     # Partial download (resume-safe, cleaned on uninstall)
└── GeoSite.dat.tmp      # Partial download (resume-safe, cleaned on uninstall)

$XDG_RUNTIME_DIR/mihomo/
└── mihomo.sock          # Unix domain socket (Linux)

/tmp/mihomo-$UID/
└── mihomo.sock          # Unix domain socket (macOS per-user)

/etc/systemd/system/          # Linux (system mode)
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
  1. Add flag=clashmeta parameter to URL
  2. GET with User-Agent: "clash-meta/v1.19.0"
     ├─ response contains "proxies:" / "mixed-port:" / "mode:"
     │   → Clash YAML, save directly ✅
     └─ non-YAML response
         → treat as raw subscription, convert
  3. If flag=clashmeta fails (HTTP error or non-YAML)
     → retry with original URL (no flag parameter)
  4. If still non-YAML
     → treat as raw subscription, convert
```

### 7.2 Conversion (vmess/base64 → Clash YAML)

- Parses `vmess://` URLs (base64-decoded JSON)
- Parses `trojan://` URLs
- Detects base64-encoded subscription lists (tries padding variants)
- Generates full Clash YAML with:
  - Proxies
  - Proxy groups: `节点选择` (select), `自动选择` (url-test)
  - DNS (fake-ip mode)
  - TUN (disabled by default, user can enable with `mihomo-cli tun on`)
  - External controller (Unix socket)

### 7.3 Config Validation

After saving, runs `mihomo -t -d {config_dir}` to validate syntax.

---

## 8. Service Management

### 8.1 Platform Support

| Platform | Service Mechanism | System Service | Per-user Service |
|----------|------------------|----------------|------------------|
| Linux    | systemd           | `/etc/systemd/system/mihomo.service` | `~/.config/systemd/user/mihomo.service` |
| macOS    | launchd           | `/Library/LaunchDaemons/io.mihomo.plist` | `~/Library/LaunchAgents/io.mihomo.plist` |
| Windows  | sc.exe / direct process | Windows Service | Direct user process |

### 8.2 Mode Detection (v3)

v3 uses **runtime-first detection**. The CLI checks active runtimes before installed-service fallbacks:
- system daemon IPC socket: `/var/run/mihomo/service.sock`
- per-user core API socket: `$XDG_RUNTIME_DIR/mihomo/mihomo.sock` on Linux, `/tmp/mihomo-$UID/mihomo.sock` on macOS
- service-manager active probes: `systemctl is-active --quiet mihomo`, `systemctl --user is-active --quiet mihomo`, `launchctl print <domain>`, `sc.exe query mihomo`

If both system and user runtimes are active, commands fail fast because v3 modes are mutually exclusive. If neither runtime is active, commands fall back to installed service artifacts; start-like commands with no state default to per-user mode.

### 8.3 Mutually Exclusive Modes

The two modes cannot coexist meaningfully:
- **System Service**: Runs as root, TUN available, one-time password for install
- **Per-user Service**: Runs as current user, TUN unavailable, no password needed

See [SPEC-architecture-v3](docs/design/SPEC-architecture-v3.md) for the full design.

### 8.4 macOS launchd Commands (Modern API)

macOS 上使用 Modern API（`bootstrap`/`bootout`/`kickstart`/`kill`），不使用已废弃的 Legacy API（`load`/`unload`/`start`/`stop`）。

| Operation | Command |
|-----------|---------|
| Install (register + start) | `launchctl bootstrap system <plist>` |
| Uninstall (stop + remove)  | `launchctl bootout system/io.mihomo` |
| Start (restart process)    | `launchctl kickstart -k system/io.mihomo` |
| Stop (terminate process)   | `launchctl kill SIGTERM system/io.mihomo` |

**Rationale**: Legacy `load`/`unload` 在 macOS 13+ 已 deprecated，与 Modern `bootout` 混用会导致服务状态不一致（plist 存在但 launchd 未注册）。详见 ADR-12。

**KeepAlive 策略**: plist 使用 `KeepAlive.Crashed=true` 而非 `KeepAlive=true`。launchd 仅在进程因崩溃信号（SIGSEGV、SIGILL 等）死亡时自动重启；`kill SIGTERM`（用户主动 stop）不会触发重启。

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
| v3 architecture: Phase 1 | 移除双实例歧义错误，自动解析到 User 模式 | `a8f2172` |
| v3 architecture: Phase 2a | historical `--root` → `--system` 全局重命名 + 术语更新 | `b9c8d91` |
| v3 architecture: Phase 2b | 收敛非 install/uninstall 命令为自动检测，公开仅保留 `--system` 覆盖 | `a3248e6` |
| v3 architecture: Phase 5 | 冲突检测 + 清晰错误消息（双服务警告、TUN 权限、启动预检） | `e2ce3b2` |
| proxy command | `eval "$(mihomo-cli proxy on/off)"` — dynamic port from API | `a5d848e` |
| parse_selector_nodes | Pure function + 5 unit tests | `44aa111` |
| start/stop/restart | Auto-detected lifecycle control; explicit public override is `--system` | `c50a3dd` |
| Flat select UX | `mihomo-cli select` — flat list across all groups, fuzzy search by node name | `51eaf20` |
| API readiness race | `wait_for_api_ready()` polls /configs after start/restart to prevent false "unresponsive" | `70b1809` |
| Geo pre-download | `ensure_geo_files()` downloads geoip.metadb/GeoSite.dat before mihomo starts — eliminates chicken-and-egg deadlock (mihomo needs proxy to reach GitHub, but isn't proxy yet). Resume support, mirror fallback, progress bar, corrupt detection. | `0d3029d`…`cfcc10a` |
| Rule management | `mihomo-cli rule add/list/remove/clear/import/export/position` — user-defined routing rules stored in `rules.yaml`, merged into `config.yaml` at start/reload with configurable insertion position (front/back). Uses serde_yaml-validated marker editing and resolved endpoint injection; fails explicitly on invalid YAML. | `5d13b36` + v3 hardening |
| Core download resume (P0-1) | Fix: unconditional `.part` size check on every `download_once` call — was gated on `attempt > 0`, breaking cross-process resume | — |
| Core download .part cleanup (P0-2) | Fix: move `remove_file(&part_path)` before decompression — was only on success path, leaking .part on GzDecoder failure | — |

### Planned

| Feature | Notes |
|---------|-------|
| Desktop notification | Notify on node switch via D-Bus/notification center |
| Rule group support | Support rule-provider / rule-groups from external files |
| Rule position per-rule | Allow individual rules to override insertion position |

## 规则管理 V3：serde_yaml 校验 + 标记区块编辑

### 版本演进

| 版本 | 实现方式 | 问题/状态 |
|------|----------|-----------|
| V1 | config.original.yaml 合并源 | 双文件维护，非原子写入 |
| V2 | 标记法 + 字符串拼接 | 字符串操作曾破坏 YAML 缩进（BUGS.md #1） |
| V3a | tree-sitter-yaml CST | 精确但引入 C parser，阻塞 Windows 交叉验证 |
| **V3b** | **serde_yaml 校验 + 标记区块编辑 + endpoint 注入** | 当前实现；避免 native parser 依赖 |

### 当前核心思路

- `YamlEditor::parse()` 使用 `serde_yaml` 先验证输入 YAML。
- 用户规则仍使用 `# === USER RULES START ===` / `# === USER RULES END ===` 标记区块，支持 front/back 插入。
- DNS `nameserver-policy` 使用 `serde_yaml::Mapping` 更新，避免手写嵌套 YAML。
- runtime controller 字段由 resolved instance endpoint 重新注入，移除旧的 `external-controller` / `external-controller-unix` / `external-controller-pipe` / `external-ui`。
- 编辑后再次用 `serde_yaml` 验证，并通过 `utils::atomic_write_file()` 原子写入。
- 不再依赖 `tree-sitter` / `tree-sitter-yaml`，降低 Windows GNU/MSVC target check 的 C toolchain 要求。

### 关键实现

- `src/yaml_editor.rs::YamlEditor`: serde_yaml 校验 + top-level key/marker 编辑，提供 `ensure_controller`, `merge_rules`, `merge_dns_policies`
- `utils::atomic_write_file()`: 原子写入

### 向后兼容

- 标记格式不变（`# === USER RULES START/END ===`）
- 现有 `rule` / `dns` 命令接口不变
- `rules.yaml` / `dns-policy.yaml` 格式不变

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

### 4. 写操作必须 Post-Validate：写完就验，无效就滚

**原则**: 所有修改 config.yaml 的操作，写入后必须立即验证 YAML 合法性（`mihomo -t` 或 `serde_yaml` 解析），验证失败则回滚到写入前的备份。不允许"写入损坏配置"这种中间状态留存。

**动机**: BUG-01（规则合并破坏 YAML 缩进）和 BUG-08（无效配置残留导致 socket 缺失）的根因都是"写完没验"。原则 2（原子化）解决了写入过程的原子性，但没有解决"写入内容本身就是错的"这个问题。

**覆盖范围**: `merge_user_config()`（规则/DNS 合并）、`save_config()`（订阅保存）、`config --fix`（配置修复）。

### 5. 测试分层：Unit（无后端）vs E2E（真实 mihomo）

**原则**: 测试分为两层。Unit 测试不依赖 mihomo binary，验证纯逻辑；E2E 测试需要真实 mihomo binary，验证完整流程（写配置 → `mihomo -t` → 启动 → socket 通信）。

**动机**: BUG-01 如果有 E2E 测试就能在开发阶段捕获——merge 规则后调用 `mihomo -t` 验证产物合法性。

**CI 策略**: Unit 每次提交必跑；E2E 在有 mihomo binary 的环境跑（`cargo test --features e2e`）。

### 6. 诊断输出结构化：`--json` 机器可读

**原则**: 关键诊断命令（`status`、`ip`）支持 `--json` 输出，提供结构化、可解析的状态信息。人类可读保持默认，`--json` 为可选 flag。

**动机**: 用户报告问题时贴 JSON 即可定位，也为未来 Agent 集成铺路。

---

## Architecture Decision Records (ADR)

### ADR-01: Unix socket over HTTP controller

**决策**: 使用 Unix socket 而非 HTTP TCP 端口作为 mihomo API 通信方式。

**路径策略**:
- Linux: `$XDG_RUNTIME_DIR/mihomo/mihomo.sock`（符合 XDG Base Directory Specification）
- macOS: `/tmp/mihomo-$UID/mihomo.sock`（macOS 不支持 XDG_RUNTIME_DIR；按 UID 隔离多用户）
- Windows user: `\\.\pipe\mihomo-$USERNAME`；system core: `\\.\pipe\mihomo-core`；system service IPC: `\\.\pipe\mihomo-service`

**理由**:
- 安全性：不暴露网络端口，仅本机访问
- 权限隔离：socket 文件权限由 mihomo 进程控制
- Linux 自动清理：用户注销时 `$XDG_RUNTIME_DIR` 自动清理
- macOS 简单可靠：`/tmp` 始终可用

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
- `crossterm` 替代 fzf 实现交互式选择（支持 j/k vim 快捷键）

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

---

## 13. Mihomo Core Binary Download

> Implemented in `installer.rs`. This section documents the download-resume-retry
> design decisions that were not captured before the two P0 bugs surfaced.

### 13.1 Architecture

```
download_mihomo()
    │
    ├─ download_with_retry(url, part_path) → Vec<u8>
    │   │
    │   └─ for attempt in 0..3:
    │       └─ download_once(client, url, part_path)
    │           │
    │           ├─ Reads .part file size → resumed_size (unconditional, even on attempt 0)
    │           ├─ If resumed_size > 0 → Range: bytes={resumed_size}-
    │           ├─ HTTP response → download_body(resp, part_path, resumed_size)
    │           │   └─ Append-mode write to .part, return read-back Vec<u8>
    │           └─ On 416 (Range Not Satisfiable) → delete .part, fresh download
    │
    ├─ std::fs::remove_file(&part_path)   ← immediate cleanup, before decompression
    ├─ Decompress (GzDecoder on Unix, ZipArchive on Windows)
    └─ Install binary, set permissions
```

### 13.2 Partial-file Lifecycle (.part)

**File name**: `{bin_path}.part` (e.g. `~/.local/bin/mihomo.part`)

**State machine**:

```
        (no file)
            │
            ▼
    ┌───────────────┐     download_body()      ┌──────────────────┐
    │    absent     │ ─────────────────────────→│   growing        │
    └───────────────┘   append-mode writes       │                  │
            ▲                                    └──────────────────┘
            │                                           │
            │                                     download_body() returns
            │                                           │
            │                                           ▼
            │                                    ┌──────────────────┐
            │                                    │   completed      │
            │                                    └──────────────────┘
            │                                           │
            │                            ┌──────────────┴──────────────┐
            │                            │                             │
            │                    decompression ok            decompression fails
            │                            │                             │
            │                    remove_file()                 remove_file()
            │                            │                             │
            └────────────────────────────┴─────────────────────────────┘
```

**Key rules**:

1. `.part` is **always** deleted immediately after `download_with_retry` returns — before decompression starts.
   - Reason: if the file is corrupt (DNS-poisoned proxy, bit-flip, truncated), keeping it would poison the next resume attempt.
2. Resume reads the `.part` file **unconditionally** on every `download_once` call, regardless of `attempt` counter.
   - Reason: a `.part` file can outlive the process (crash, SIGKILL, power loss). The `attempt` counter is process-local and resets to 0 on restart.
3. GitHub Releases assets for a given tag are immutable — resuming across process restarts will NOT receive different content.
4. Only one `.part` file exists per binary path. No lock files needed because only one `download_mihomo` call is active at a time (single-threaded CLI).

### 13.3 Retry Strategy

- **Max retries**: 3 (configurable via `max_retries`)
- **Backoff**: 1s → 2s → 4s (exponential, `1 << (attempt - 1)`)
- **Scope**: retry loop in `download_with_retry` wraps HTTP errors + network failures.
  - `download_once` returns `Err` → retry.
  - `download_once` returns `Ok` → done (even if bytes are corrupt — decompression will catch it, and `.part` cleanup ensures next run starts fresh).

### 13.4 Error Paths

| Scenario | Behavior | .part state after |
|----------|----------|-------------------|
| Network drops mid-download | Retry with resume (Range from saved .part size) | Partial, grows on retry |
| Server returns 416 (Range not satisfiable) | Delete .part, retry from zero | Deleted, then fresh download |
| All retries exhausted | Error propagated to caller | Partial .part left on disk (clean slate for next `download_mihomo` call) |
| Download succeeds, decompression fails | Error, .part deleted before decompress | Deleted (next call starts fresh) |
| Download succeeds, decompression ok | Success, .part deleted after download | Deleted |

### 13.5 Design Decisions

**ADR-07: Unconditional resume check (2026-07-13)**

**Before**: `resumed_size` was set to 0 when `attempt == 0`, ignoring any `.part` file from a previous process.

**After**: `resumed_size = std::fs::metadata(part_path).map(|m| m.len()).unwrap_or(0)` — always checks.

**Reason**: The `attempt` counter is a per-process retry index. It has no relationship to whether a `.part` file exists from a prior invocation. Bug P0-1.

---

**ADR-08: .part cleanup before decompression (2026-07-13)**

**Before**: `remove_file(&part_path)` executed only after successful decompression. If `GzDecoder` or `ZipArchive` failed, `.part` was left on disk, poisoning the next resume.

**After**: `remove_file(&part_path)` runs immediately after `download_with_retry` returns, before any decompression.

**Reason**: If decompression fails, the downloaded bytes are corrupt. Resuming from corrupt bytes is pointless — next run should start fresh. Bug P0-2.

---

**ADR-09: No SHA256 checksum (deferred)**

**Decision**: Do NOT add SHA256 verification for now.

**Reason**: GitHub Releases serve over HTTPS + CDN, and `Content-Length` matching + GzDecoder checksum (gzip has built-in CRC32) provide a reasonable integrity baseline. Adding a checksum file separate download adds a failure point (what if the checksum file download fails but the binary succeeds?). Revisit if users report "decompression failed" errors.

---

**ADR-10: YAML editing strategy evolution (2026-07-15, revised 2026-07-26)**

**Before**: 纯字符串操作容易破坏 YAML 缩进和格式。

**Interim**: 曾引入 `tree-sitter-yaml` 做 CST 编辑，但这带来 C parser/native build 依赖，增加 Windows GNU/MSVC target check 成本。

**Current**: 使用 `serde_yaml` 做输入/输出校验；规则区块保留 marker 编辑；DNS policy 使用 `serde_yaml::Mapping` 更新；runtime controller 由 resolved instance endpoint 统一注入。

**Benefits**:
- 失败时显式报错，不静默生成无效 YAML
- 移除 `tree-sitter` / `tree-sitter-yaml` native 依赖，改善 Windows target 验证
- 保留用户规则 marker 的可读性和可维护性
- endpoint 注入与 v3 system/user runtime-first 解析一致

**Trade-offs**:
- 不再承诺 CST 级格式保持；DNS policy 更新可能按 serde_yaml 序列化规范化局部结构
- 顶层 key/marker 编辑仍需保持输入 YAML 为常规 block mapping 格式

**Implementation**: `src/yaml_editor.rs` 提供 `YamlEditor`，封装 serde_yaml 校验和有限文本编辑操作。


**ADR-11: Remove fallback path, fail explicitly (2026-07-15)**

**Decision**: 移除不安全的静默 fallback 路径；YAML 编辑/校验失败必须显式报错。

**Before**: YAML 解析/编辑失败时，静默回退到旧的字符串拼接方式。

**Problem**: 
- 隐藏 bug（fallback 路径本身有缩进问题）
- 用户无感知，不知道发生了什么
- 增加代码复杂度（维护两套逻辑）

**After**: YAML 校验或编辑失败时直接报错，给出明确的错误信息和修复建议：

```
Error: Failed to edit config.yaml: <具体原因>

You can:
1. Manually edit ~/.config/mihomo/config.yaml to fix the issue
2. Report this bug at: https://github.com/CNCSMonster/mihomo-cli/issues
```

**Rationale**: 
- 符合 CLI 工具最佳实践：宁可明确失败，不要静默降级
- 错误透明，用户/维护者能快速定位问题
- 代码更简洁，只维护一套逻辑

**User Action**: 如果是用户操作导致的配置问题，用户可以手动编辑文件修复；如果是工具 bug，用户报告 issue 后临时手动修改配置。

**ADR-12: macOS launchd 统一使用 Modern API (2026-07-21)**

**Decision**: macOS 服务管理统一使用 Modern API（bootstrap/bootout/kickstart/kill），废弃 Legacy API（load/unload/start/stop）。

**Problem**:
- install 用 `launchctl load`（Legacy），uninstall 用 `launchctl bootout`（Modern）
- 两套 API 操作不同的内部状态，混用导致 uninstall → install → start 流程失败
- macOS 15 Sequoia 上 `load` 行为不可靠

**Before**:
- install: `launchctl load <plist>`
- uninstall: `launchctl bootout system <plist>`
- start: `launchctl start io.mihomo`
- stop: `launchctl stop io.mihomo`

**After**:
- install: `launchctl bootstrap system <plist>`
- uninstall: `launchctl bootout system/io.mihomo`
- start: `launchctl kickstart -k system/io.mihomo`
- stop: `launchctl kill SIGTERM system/io.mihomo`

**Rationale**:
- Modern API 从 macOS 10.10 Yosemite 起可用，覆盖所有合理支持的版本
- `bootstrap`/`bootout` 是配对操作，保证注册/注销状态一致
- `kickstart -k` 强制重启进程，比 `start` 更可靠（`start` 仅在服务未运行时启动）
- `kill SIGTERM` 优雅终止；plist 使用 `KeepAlive.Crashed=true`，SIGTERM 不是崩溃信号，launchd 不会重启进程

### 13.6 Geo File Download (Separate Path)

Geo files (`geoip.metadb`, `GeoSite.dat`) use a **different** strategy from the core binary:

| Aspect | Core binary | Geo files |
|--------|-------------|-----------|
| Resume | HTTP Range on `.part` | Per-URL fresh start (`.tmp` deleted on mirror switch) |
| Retry | Same URL 3x with backoff | Different URLs (GitHub → jsDelivr → ghproxy), each 2x retry |
| Validation | GzDecoder CRC32 (implicit) | Size check + first-byte + `mihomo -t` |
| Temp file | `.part`, deleted before decompress | `.tmp`, renamed to target on success |

Geo files use `.tmp` + `rename` (atomic) instead of `.part` + delete. This is because geo downloads are smaller (~8 MB) and the mirror fallback strategy means a partial file from mirror A is invalid for mirror B.

