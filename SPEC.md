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
| `mihomo-cli completions` | Generate shell completions | None |

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
├── config.yaml          # Clash YAML configuration
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

### Planned

| Feature | Notes |
|---------|-------|
| Config backup/restore | Snapshot config before risky operations |
| Desktop notification | Notify on node switch via D-Bus/notification center |
