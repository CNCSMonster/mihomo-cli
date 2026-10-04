//! Issue #022 子问题 A：`doctor` 的 P2 本地解析器探测（Local Resolver Probe）。
//!
//! 语义依据：`docs/drafts/SPEC-doctor-local-resolver-probe.draft.md` 的已确认决策
//! - **D1** 默认探测（opt-out，`doctor --no-probe-dns` 关闭）；
//! - **D2** 仅当**运行态 TUN 生效**且**运行配置**（launched/active，非 intent）为
//!   `dns.enhanced-mode: fake-ip` 时探测，否则跳过（review MAJOR-7 修订）；
//! - **D3** 探测目标是**系统解析器**（`/etc/resolv.conf` 的 nameserver）——这是被观测对象本身，
//!   也是 #022 实测中唯一给出决定性证据的检查（`resolvectl query` 返回真实地址 → 未进 TUN）；
//! - **D4** 结果只作为 `doctor` 的独立诊断字段，不进入 Health / attestation / `status`。
//!
//! 硬边界：
//! - 只向 `/etc/resolv.conf` 中的 **loopback** 解析器发**单次**查询（D3；非 loopback
//!   显式拒绝并给出原因，不转而探测任意地址），总超时 ≤ 2s，一次尝试，超时即放弃；
//! - 不 sudo、不写文件、不触发 recovery，不访问 HTTP/代理/订阅；
//! - 探测不得回填 `StatusSnapshot`，只能生成一条独立的 `DoctorCheck`。
//!
//! 证据分级：分类规则本身依赖 `dns.fake-ip-range`（mihomo fake-ip 只会给域名分配该段地址），
//! 因此「返回地址落在该段 → 已经过 mihomo」为 L1 推论；「返回真实地址 → 绕过 mihomo」同样
//! 为 L1（同一查询经 TUN 必然得到 fake-ip）。

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

/// 探测用域名：IANA 保留域名，稳定可解析，非用户业务域名。
///
/// 事务 ID 每次随机（`rand::random()`），域名固定——域名策略的取舍见 SPEC 草案 **D5**（待决策）：
/// 固定域名保证上游必有 A 应答（绕过态稳定判 ❌），代价是可能受系统解析缓存与
/// `fake-ip-filter` 精确命中影响产生假 ❌；随机子域名能消除缓存影响，但对无通配的
/// `example.com` 会让绕过态退化为 NXDOMAIN → ❓。
pub(crate) const PROBE_DOMAIN: &str = "example.com";

/// 单次探测总超时（SPEC 草案：≤ 2s，单次尝试）。
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_millis(2000);

/// mihomo 未显式配置 `fake-ip-range` 时的官方默认值。
pub(crate) const DEFAULT_FAKE_IP_RANGE: &str = "198.18.0.1/16";

const DNS_TYPE_A: u16 = 1;
const DNS_CLASS_IN: u16 = 1;

/// fake-ip 网段（`addr/prefix`，允许非网络对齐的写法，例如 `198.18.0.1/16`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FakeIpRange {
    network: u32,
    prefix: u8,
}

impl fmt::Display for FakeIpRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", Ipv4Addr::from(self.network), self.prefix)
    }
}

impl FakeIpRange {
    /// 解析 `a.b.c.d/prefix`；任何格式错误都返回 `None`（调用方降级为 unknown）。
    pub(crate) fn parse(spec: &str) -> Option<Self> {
        let (addr, prefix) = spec.trim().split_once('/')?;
        let addr: Ipv4Addr = addr.trim().parse().ok()?;
        let prefix: u8 = prefix.trim().parse().ok()?;
        if prefix > 32 {
            return None;
        }
        Some(Self {
            network: u32::from(addr),
            prefix,
        })
    }

    fn mask(&self) -> u32 {
        if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - self.prefix)
        }
    }

    pub(crate) fn contains(&self, addr: Ipv4Addr) -> bool {
        let mask = self.mask();
        (u32::from(addr) & mask) == (self.network & mask)
    }
}

/// 探测结论（SPEC 草案 §1 分类表）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeVerdict {
    /// 返回地址落在 fake-ip 范围内 → 系统解析路径经过 mihomo。
    RoutedThroughMihomo,
    /// 返回真实地址 → 系统解析路径绕过 mihomo（#022 的失效签名）。
    BypassesMihomo,
    /// NXDOMAIN / SERVFAIL / 超时 / 无法确定 fake-ip 范围 → 不告警、不阻断。
    Unknown(String),
}

/// 构造一条标准的递归 A 查询（RD=1）。
pub(crate) fn build_query(id: u16, name: &str) -> Vec<u8> {
    let mut msg = Vec::with_capacity(64);
    msg.extend_from_slice(&id.to_be_bytes());
    msg.extend_from_slice(&0x0100u16.to_be_bytes()); // flags: RD
    msg.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    msg.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    msg.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    msg.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    for label in name.trim_matches('.').split('.') {
        msg.push(label.len() as u8);
        msg.extend_from_slice(label.as_bytes());
    }
    msg.push(0); // root label
    msg.extend_from_slice(&DNS_TYPE_A.to_be_bytes());
    msg.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());
    msg
}

fn read_u16(msg: &[u8], offset: usize) -> Result<u16, String> {
    let bytes = msg
        .get(offset..offset + 2)
        .ok_or_else(|| "DNS message truncated".to_string())?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// 跳过一个域名（支持压缩指针），返回结束后的偏移。
fn skip_name(msg: &[u8], mut offset: usize) -> Result<usize, String> {
    loop {
        let len = *msg
            .get(offset)
            .ok_or_else(|| "DNS name truncated".to_string())?;
        if len == 0 {
            return Ok(offset + 1);
        }
        if len & 0xC0 == 0xC0 {
            // 压缩指针：占 2 字节，之后域名结束。
            msg.get(offset + 1)
                .ok_or_else(|| "DNS compression pointer truncated".to_string())?;
            return Ok(offset + 2);
        }
        offset = offset
            .checked_add(1 + len as usize)
            .ok_or_else(|| "DNS name length overflow".to_string())?;
        if offset >= msg.len() {
            return Err("DNS name truncated".to_string());
        }
    }
}

/// 解析应答中的 A 记录。
///
/// 返回 `Err` 表示报文本身有问题（不匹配的事务 ID、非响应、截断等）；
/// 返回 `Ok(vec![])` 表示报文合法但没有 A 记录（NXDOMAIN / 纯 CNAME 等）。
pub(crate) fn parse_a_records(msg: &[u8], expected_id: u16) -> Result<Vec<Ipv4Addr>, String> {
    if msg.len() < 12 {
        return Err("DNS response too short".to_string());
    }
    let id = read_u16(msg, 0)?;
    if id != expected_id {
        return Err(format!(
            "DNS transaction id mismatch ({id} != {expected_id})"
        ));
    }
    let flags = read_u16(msg, 2)?;
    if flags & 0x8000 == 0 {
        return Err("DNS message is not a response".to_string());
    }
    let rcode = flags & 0x000F;
    let qdcount = read_u16(msg, 4)? as usize;
    let ancount = read_u16(msg, 6)? as usize;

    let mut offset = 12;
    for _ in 0..qdcount {
        offset = skip_name(msg, offset)?;
        offset = offset
            .checked_add(4)
            .ok_or_else(|| "DNS question truncated".to_string())?;
        if offset > msg.len() {
            return Err("DNS question truncated".to_string());
        }
    }

    let mut answers = Vec::new();
    // rcode != 0（NXDOMAIN/SERVFAIL）时没有可用答案，直接返回空列表。
    if rcode != 0 {
        return Ok(answers);
    }
    for _ in 0..ancount {
        offset = skip_name(msg, offset)?;
        let rtype = read_u16(msg, offset)?;
        let rclass = read_u16(msg, offset + 2)?;
        let rdlength = read_u16(msg, offset + 8)? as usize;
        let rdata = offset + 10;
        offset = rdata
            .checked_add(rdlength)
            .ok_or_else(|| "DNS record length overflow".to_string())?;
        if offset > msg.len() {
            return Err("DNS record truncated".to_string());
        }
        if rtype == DNS_TYPE_A && rclass == DNS_CLASS_IN && rdlength == 4 {
            answers.push(Ipv4Addr::new(
                msg[rdata],
                msg[rdata + 1],
                msg[rdata + 2],
                msg[rdata + 3],
            ));
        }
    }
    Ok(answers)
}

/// 把一次成功查询的结果与 fake-ip 范围比对，得出分类。
pub(crate) fn classify(answers: &[Ipv4Addr], range: &FakeIpRange) -> ProbeVerdict {
    match answers.first() {
        None => ProbeVerdict::Unknown("解析器未返回 A 记录（NXDOMAIN/空应答）".to_string()),
        Some(addr) if range.contains(*addr) => ProbeVerdict::RoutedThroughMihomo,
        Some(_) => ProbeVerdict::BypassesMihomo,
    }
}

/// 读取系统解析器地址（`/etc/resolv.conf` 的 `nameserver`）。
#[cfg(unix)]
pub(crate) fn system_resolver_addrs() -> Vec<SocketAddr> {
    let Ok(content) = std::fs::read_to_string("/etc/resolv.conf") else {
        return Vec::new();
    };
    system_resolver_addrs_from(&content)
}

/// 从 `resolv.conf` 文本中提取 nameserver（供测试与解析共用）。
pub(crate) fn system_resolver_addrs_from(content: &str) -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("nameserver") else {
            continue;
        };
        let token = rest.trim();
        if token.is_empty() {
            continue;
        }
        // 只接受 IPv4/IPv6 字面量；IPv6 用 53 端口。
        if let Ok(ip) = token.parse::<std::net::IpAddr>() {
            let addr = SocketAddr::new(ip, 53);
            if !addrs.contains(&addr) {
                addrs.push(addr);
            }
        }
    }
    addrs
}

/// P2 探测只允许指向 loopback 解析器（SPEC draft §1 关键澄清）。
///
/// 没有 loopback 解析器时返回带原因的错误，调用方必须把原因**显式**呈现给用户，
/// 而不是静默跳过——否则“探测被跳过”本身又会成为不可见状态。
pub(crate) fn loopback_resolver(addrs: &[SocketAddr]) -> Result<SocketAddr, String> {
    match addrs.iter().find(|addr| addr.ip().is_loopback()) {
        Some(addr) => Ok(*addr),
        None if addrs.is_empty() => Err("未在 /etc/resolv.conf 中找到 nameserver".to_string()),
        None => Err(format!(
            "系统解析器 {} 非 loopback，按只读探测边界跳过（可用 --no-probe-dns 明确关闭该项）",
            addrs
                .iter()
                .map(|addr| addr.ip().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// 向单个解析器发起一次有界 A 查询。
pub(crate) async fn query_a(
    addr: SocketAddr,
    domain: &str,
    id: u16,
    timeout: Duration,
) -> Result<Vec<Ipv4Addr>, String> {
    let socket = tokio::net::UdpSocket::bind(if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .await
    .map_err(|e| format!("无法绑定本地 UDP socket: {e}"))?;
    socket
        .connect(addr)
        .await
        .map_err(|e| format!("无法连接解析器 {addr}: {e}"))?;
    socket
        .send(&build_query(id, domain))
        .await
        .map_err(|e| format!("查询发送失败: {e}"))?;

    let mut buf = [0u8; 1500];
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!("查询超时（>{timeout:?}）"));
        }
        match tokio::time::timeout(remaining, socket.recv(&mut buf)).await {
            Ok(Ok(len)) => match parse_a_records(&buf[..len], id) {
                Ok(answers) => return Ok(answers),
                // 同一 socket 上可能收到与本次查询无关的旧报文，继续等。
                Err(err) if err.contains("transaction id mismatch") => continue,
                Err(err) => return Err(err),
            },
            Ok(Err(err)) => return Err(format!("查询接收失败: {err}")),
            Err(_) => return Err(format!("查询超时（>{timeout:?}）")),
        }
    }
}

/// 执行一次系统解析器探测：读取 resolver → 单次查询 → 分类。
///
/// 任何一步失败都降级为 `ProbeVerdict::Unknown`，绝不让 `doctor` 失败。
#[cfg(unix)]
pub(crate) async fn probe_system_resolver(range: &FakeIpRange) -> ProbeVerdict {
    let addrs = system_resolver_addrs();
    let addr = match loopback_resolver(&addrs) {
        Ok(addr) => addr,
        Err(reason) => return ProbeVerdict::Unknown(reason),
    };
    let id: u16 = rand::random();
    match query_a(addr, PROBE_DOMAIN, id, PROBE_TIMEOUT).await {
        Ok(answers) => classify(&answers, range),
        Err(err) => ProbeVerdict::Unknown(err),
    }
}

/// 从 **Core 实际启动的运行配置**文本中读取 `dns` 侧的探测前置条件（D2）与 fake-ip 范围
/// （review MAJOR-7：intent 可能尚未应用，拿 intent 当判据会把“经过 mihomo”误判成绕过）。
///
/// 返回 `None` 表示不满足探测条件（非 fake-ip 模式 / 配置不可解析）。
pub(crate) fn probe_precondition(config_text: &str) -> Option<FakeIpRange> {
    let doc: serde_yaml::Value = serde_yaml::from_str(config_text).ok()?;
    let dns = doc.get("dns")?;
    let mode = dns.get("enhanced-mode")?.as_str()?;
    if mode != "fake-ip" {
        return None;
    }
    let range = dns
        .get("fake-ip-range")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_FAKE_IP_RANGE);
    FakeIpRange::parse(range)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一条应答报文（可选携带压缩指针域名），用于解析器测试。
    fn build_response(id: u16, answers: &[Ipv4Addr], compress: bool) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(&id.to_be_bytes());
        msg.extend_from_slice(&0x8180u16.to_be_bytes()); // QR + RD + RA, rcode 0
        msg.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
        msg.extend_from_slice(&(answers.len() as u16).to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        // question
        for label in PROBE_DOMAIN.split('.') {
            msg.push(label.len() as u8);
            msg.extend_from_slice(label.as_bytes());
        }
        msg.push(0);
        msg.extend_from_slice(&DNS_TYPE_A.to_be_bytes());
        msg.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());
        // answers
        for addr in answers {
            if compress {
                msg.extend_from_slice(&0xC00Cu16.to_be_bytes()); // 指向 question 中的域名
            } else {
                for label in PROBE_DOMAIN.split('.') {
                    msg.push(label.len() as u8);
                    msg.extend_from_slice(label.as_bytes());
                }
                msg.push(0);
            }
            msg.extend_from_slice(&DNS_TYPE_A.to_be_bytes());
            msg.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());
            msg.extend_from_slice(&0u32.to_be_bytes()); // TTL (4 bytes)
            msg.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
            msg.extend_from_slice(&addr.octets());
        }
        msg
    }

    #[test]
    fn fake_ip_range_parses_and_matches_reference_range() {
        let range = FakeIpRange::parse("198.18.0.1/16").unwrap();
        assert_eq!(range.to_string(), "198.18.0.1/16");
        assert!(range.contains(Ipv4Addr::new(198, 18, 0, 1)));
        assert!(range.contains(Ipv4Addr::new(198, 18, 200, 9)));
        assert!(!range.contains(Ipv4Addr::new(172, 217, 115, 4)));
        assert!(!range.contains(Ipv4Addr::new(28, 0, 0, 4)));
    }

    #[test]
    fn fake_ip_range_parses_legacy_28_over_8() {
        // Issue #022 实测环境的旧默认值，必须能正确识别其派生地址。
        let range = FakeIpRange::parse("28.0.0.1/8").unwrap();
        assert!(range.contains(Ipv4Addr::new(28, 0, 0, 4)));
        assert!(!range.contains(Ipv4Addr::new(198, 18, 0, 1)));
    }

    #[test]
    fn fake_ip_range_rejects_malformed_specs() {
        assert!(FakeIpRange::parse("198.18.0.1").is_none());
        assert!(FakeIpRange::parse("198.18.0.1/33").is_none());
        assert!(FakeIpRange::parse("example.com/16").is_none());
        assert!(FakeIpRange::parse("").is_none());
    }

    #[test]
    fn query_builder_encodes_standard_recursion_query() {
        let query = build_query(0x1234, "example.com");
        assert_eq!(&query[0..2], &[0x12, 0x34]);
        assert_eq!(&query[2..4], &[0x01, 0x00]); // RD
        assert_eq!(&query[4..6], &[0x00, 0x01]); // QDCOUNT=1
                                                 // QNAME: 7 example 3 com 0
        assert_eq!(query[12], 7);
        assert_eq!(&query[13..20], b"example");
        assert_eq!(query[20], 3);
        assert_eq!(&query[21..24], b"com");
        assert_eq!(query[24], 0);
        assert_eq!(&query[25..27], &[0x00, 0x01]); // TYPE A
        assert_eq!(&query[27..29], &[0x00, 0x01]); // CLASS IN
        assert_eq!(query.len(), 29);
    }

    #[test]
    fn parse_a_records_reads_answers_with_and_without_compression() {
        let id = 0x4242;
        let addr = Ipv4Addr::new(198, 18, 0, 7);
        let plain = build_response(id, &[addr], false);
        assert_eq!(parse_a_records(&plain, id).unwrap(), vec![addr]);

        let compressed = build_response(id, &[addr], true);
        assert_eq!(parse_a_records(&compressed, id).unwrap(), vec![addr]);
    }

    #[test]
    fn parse_a_records_rejects_wrong_id_and_non_responses() {
        let response = build_response(7, &[Ipv4Addr::LOCALHOST], false);
        assert!(parse_a_records(&response, 8)
            .unwrap_err()
            .contains("id mismatch"));
        assert!(parse_a_records(b"short", 7).is_err());

        let mut query = build_query(7, PROBE_DOMAIN);
        assert!(parse_a_records(&query, 7)
            .unwrap_err()
            .contains("not a response"));
        // 同事务 ID 的查询报文也必须被拒绝（不得被当成应答）。
        query.truncate(4);
        assert!(parse_a_records(&query, 7).is_err());
    }

    #[test]
    fn parse_a_records_treats_nxdomain_as_empty_answer() {
        let mut msg = build_response(9, &[], false);
        // flags: QR + rcode 3 (NXDOMAIN)
        msg[2] = 0x81;
        msg[3] = 0x83;
        assert_eq!(parse_a_records(&msg, 9).unwrap(), Vec::<Ipv4Addr>::new());
    }

    #[test]
    fn classify_distinguishes_fake_ip_real_ip_and_empty() {
        let range = FakeIpRange::parse("198.18.0.1/16").unwrap();
        assert_eq!(
            classify(&[Ipv4Addr::new(198, 18, 0, 4)], &range),
            ProbeVerdict::RoutedThroughMihomo
        );
        assert_eq!(
            classify(&[Ipv4Addr::new(172, 217, 115, 4)], &range),
            ProbeVerdict::BypassesMihomo
        );
        assert!(matches!(classify(&[], &range), ProbeVerdict::Unknown(_)));
    }

    #[test]
    fn probe_precondition_requires_fake_ip_mode() {
        let fake =
            "dns:\n  enable: true\n  enhanced-mode: fake-ip\n  fake-ip-range: 198.18.0.1/16\n";
        assert!(probe_precondition(fake).is_some());

        // 未配置 fake-ip-range 时使用官方默认值。
        let no_range = "dns:\n  enhanced-mode: fake-ip\n";
        assert_eq!(
            probe_precondition(no_range).unwrap().to_string(),
            DEFAULT_FAKE_IP_RANGE
        );

        assert!(probe_precondition("dns:\n  enhanced-mode: redir-host\n").is_none());
        assert!(probe_precondition("mixed-port: 7890\n").is_none());
        assert!(probe_precondition("not: [valid").is_none());
    }

    #[tokio::test]
    async fn probe_gives_up_within_timeout_when_resolver_silent() {
        tokio::time::timeout(Duration::from_secs(3), async {
            // 绑定一个永不回复的本地 socket，验证单次查询有界返回。
            let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let addr = silent.local_addr().unwrap();
            let started = std::time::Instant::now();
            let err = query_a(addr, PROBE_DOMAIN, 1, Duration::from_millis(150))
                .await
                .unwrap_err();
            assert!(err.contains("超时"), "unexpected error: {err}");
            assert!(started.elapsed() < Duration::from_secs(2));
        })
        .await
        .expect("probe must be bounded");
    }

    #[tokio::test]
    async fn single_probe_query_roundtrip_against_local_resolver() {
        tokio::time::timeout(Duration::from_secs(5), async {
            // 起一个最小 DNS 服务端：回一个 fake-ip 应答，验证端到端编码/解码。
            let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let addr = server.local_addr().unwrap();
            let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            client.connect(addr).await.unwrap();
            let id: u16 = 0x5155;
            client.send(&build_query(id, PROBE_DOMAIN)).await.unwrap();

            let mut buf = [0u8; 1500];
            let (len, peer) = server.recv_from(&mut buf).await.unwrap();
            let query = &buf[..len];
            assert_eq!(u16::from_be_bytes([query[0], query[1]]), id);
            let response = build_response(id, &[Ipv4Addr::new(198, 18, 0, 9)], false);
            server.send_to(&response, peer).await.unwrap();

            let mut rbuf = [0u8; 1500];
            let rlen = client.recv(&mut rbuf).await.unwrap();
            assert_eq!(
                parse_a_records(&rbuf[..rlen], id).unwrap(),
                vec![Ipv4Addr::new(198, 18, 0, 9)]
            );
        })
        .await
        .expect("local resolver roundtrip must complete");
    }
}

#[cfg(test)]
mod resolver_selection_tests {
    use super::*;

    #[test]
    fn parses_nameservers_and_skips_garbage_lines() {
        let content = "\
# Generated by resolvconf\nsearch example.com\nnameserver 127.0.0.53\nnameserver 10.0.0.1\nnameserver not-an-ip\nnameserver 127.0.0.53\n";
        let addrs = system_resolver_addrs_from(content);
        assert_eq!(
            addrs,
            vec![
                SocketAddr::from(([127, 0, 0, 53], 53)),
                SocketAddr::from(([10, 0, 0, 1], 53)),
            ],
            "loopback 与局域网解析器按文件顺序去重保留"
        );
    }

    #[test]
    fn prefers_loopback_resolver_for_read_only_probe() {
        let addrs = system_resolver_addrs_from("nameserver 10.0.0.1\nnameserver 127.0.0.53\n");
        assert_eq!(
            loopback_resolver(&addrs).expect("loopback resolver"),
            SocketAddr::from(([127, 0, 0, 53], 53)),
            "即使 loopback 不在首位也必须选中它"
        );
    }

    #[test]
    fn refuses_non_loopback_resolver_with_explicit_reason() {
        let addrs = system_resolver_addrs_from("nameserver 192.0.2.53\n");
        let err = loopback_resolver(&addrs).expect_err("must refuse non-loopback");
        assert!(
            err.contains("非 loopback") && err.contains("192.0.2.53"),
            "原因必须包含被跳过的解析器：{err}"
        );
    }

    #[test]
    fn reports_missing_nameserver_separately_from_non_loopback() {
        let err = loopback_resolver(&[]).expect_err("must report");
        assert!(err.contains("nameserver"), "{err}");
    }
}
