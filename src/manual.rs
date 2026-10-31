//! Embedded user manual: the binary carries `docs/user-guide/` so users can read
//! the full manual without a repository checkout.
//!
//! Single source of truth is `docs/user-guide/README.md`'s `manual-topics`
//! section; the slug/title table below must stay identical to it and the
//! consistency is asserted by the unit tests in this module. Content is embedded
//! at compile time (`include_str!`), so rendering performs no filesystem read,
//! no network access and no writes.

pub(crate) const INDEX_SLUG: &str = "index";

const INDEX_CONTENT: &str = include_str!("../docs/user-guide/README.md");

pub(crate) struct Topic {
    pub(crate) slug: &'static str,
    pub(crate) title: &'static str,
    pub(crate) content: &'static str,
}

/// Order and titles mirror the `manual-topics` table in the manual index.
pub(crate) const TOPICS: &[Topic] = &[
    Topic {
        slug: "index",
        title: "mihomo-cli 用户手册",
        content: INDEX_CONTENT,
    },
    Topic {
        slug: "quickstart",
        title: "快速开始",
        content: include_str!("../docs/user-guide/quickstart.md"),
    },
    Topic {
        slug: "json",
        title: "AI/脚本 JSON 输出（第一阶段）",
        content: include_str!("../docs/user-guide/json.md"),
    },
    Topic {
        slug: "install",
        title: "安装、升级与服务控制",
        content: include_str!("../docs/user-guide/install.md"),
    },
    Topic {
        slug: "upgrade",
        title: "升级 mihomo-cli",
        content: include_str!("../docs/user-guide/upgrade.md"),
    },
    Topic {
        slug: "subscription",
        title: "订阅、配置、规则与备份",
        content: include_str!("../docs/user-guide/subscription.md"),
    },
    Topic {
        slug: "nodes",
        title: "节点、连接与出口",
        content: include_str!("../docs/user-guide/nodes.md"),
    },
    Topic {
        slug: "tun",
        title: "TUN 透明代理",
        content: include_str!("../docs/user-guide/tun.md"),
    },
    Topic {
        slug: "override",
        title: "高级配置（override.yaml）",
        content: include_str!("../docs/user-guide/override.md"),
    },
    Topic {
        slug: "troubleshooting",
        title: "平台说明与故障排查",
        content: include_str!("../docs/user-guide/troubleshooting.md"),
    },
    Topic {
        slug: "commands",
        title: "命令索引",
        content: include_str!("../docs/user-guide/commands.md"),
    },
    Topic {
        slug: "glossary",
        title: "术语",
        content: include_str!("../docs/user-guide/glossary.md"),
    },
];

pub(crate) fn topics() -> &'static [Topic] {
    TOPICS
}

pub(crate) fn find(slug: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|topic| topic.slug == slug)
}

pub(crate) fn render(slug: &str) -> Option<&'static str> {
    find(slug).map(|topic| topic.content)
}

pub(crate) fn render_index() -> &'static str {
    INDEX_CONTENT
}

pub(crate) fn list() -> Vec<(&'static str, &'static str)> {
    topics()
        .iter()
        .map(|topic| (topic.slug, topic.title))
        .collect()
}

pub(crate) fn list_lines() -> Vec<String> {
    list()
        .iter()
        .map(|(slug, title)| format!("{slug}\t{title}"))
        .collect()
}

pub(crate) fn json_topic(slug: &str, content: &str) -> serde_json::Value {
    serde_json::json!({ "topic": slug, "content": content })
}

pub(crate) fn json_list() -> serde_json::Value {
    let topics: Vec<serde_json::Value> = list()
        .iter()
        .map(|(slug, title)| serde_json::json!({ "slug": slug, "title": title }))
        .collect();
    serde_json::json!({ "topics": topics })
}

pub(crate) fn available_topics() -> String {
    topics()
        .iter()
        .map(|topic| topic.slug)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn run(list: bool, slug: Option<&str>, json: bool) -> anyhow::Result<()> {
    if list {
        if json {
            return crate::print_json_envelope("manual", json_list(), Vec::new());
        }
        for line in list_lines() {
            println!("{line}");
        }
        return Ok(());
    }
    let (slug, content) = match slug {
        None => (INDEX_SLUG, render_index()),
        Some(slug) => match render(slug) {
            Some(content) => (slug, content),
            None => {
                report_unknown_topic(slug, json);
                std::process::exit(2);
            }
        },
    };
    if json {
        return crate::print_json_envelope("manual", json_topic(slug, content), Vec::new());
    }
    print!("{content}");
    Ok(())
}

/// Unknown topic is a usage error, so it follows clap's exit code 2 instead of
/// the generic command failure code 1. The suggestion goes to stderr to keep
/// stdout reserved for manual content; with `--json` stdout still carries a
/// single error envelope.
fn report_unknown_topic(slug: &str, json: bool) {
    eprintln!("Unknown manual topic: {slug}");
    eprintln!("Available topics: {}", available_topics());
    eprintln!("Run `mihomo-cli manual --list` for the full topic list.");
    if json {
        let envelope =
            crate::json_error_envelope("manual", &format!("unknown manual topic: {slug}"));
        if let Ok(text) = serde_json::to_string_pretty(&envelope) {
            println!("{text}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const README: &str = include_str!("../docs/user-guide/README.md");
    const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

    fn readme_topic_rows() -> Vec<(String, String, String)> {
        let start = README
            .find("<!-- manual-topics -->")
            .expect("manual index must keep the manual-topics marker");
        let rest = &README[start..];
        let end = rest
            .find("<!-- /manual-topics -->")
            .expect("manual-topics marker must be closed");
        let mut rows = Vec::new();
        for line in rest[..end].lines() {
            let line = line.trim();
            if !line.starts_with('|') {
                continue;
            }
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            if cells.len() != 3 {
                panic!("unexpected manual-topics row: {line}");
            }
            if cells[0] == "slug" || cells.iter().all(|cell| cell.starts_with("---")) {
                continue;
            }
            rows.push((
                cells[0].to_string(),
                cells[1].to_string(),
                cells[2].to_string(),
            ));
        }
        rows
    }

    #[test]
    fn manual_topic_table_matches_the_embedded_topics() {
        let rows = readme_topic_rows();
        assert_eq!(rows.len(), 12, "manual index must list 12 topics");
        assert_eq!(rows.len(), TOPICS.len(), "embedded topic table is stale");

        for (row, topic) in rows.iter().zip(TOPICS) {
            assert_eq!(row.0, topic.slug, "slug order/name drifted from README");
            assert_eq!(row.1, topic.title, "title drifted from README");
            let path = std::path::Path::new(MANIFEST_DIR)
                .join("docs/user-guide")
                .join(&row.2);
            let on_disk = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            assert_eq!(
                on_disk, topic.content,
                "embedded content for `{}` does not match {}",
                topic.slug, row.2
            );
        }
    }

    #[test]
    fn every_topic_is_a_non_empty_markdown_document() {
        for topic in TOPICS {
            assert!(!topic.content.trim().is_empty(), "{} is empty", topic.slug);
            let first_line = topic.content.lines().next().unwrap_or_default();
            assert!(
                first_line.starts_with("# "),
                "{} must start with a markdown heading, got: {first_line}",
                topic.slug
            );
            assert!(
                topic.content.ends_with('\n'),
                "{} must keep its trailing newline for byte-exact output",
                topic.slug
            );
        }
    }

    #[test]
    fn slugs_and_titles_are_unique() {
        for (index, topic) in TOPICS.iter().enumerate() {
            assert!(
                TOPICS[..index].iter().all(|seen| seen.slug != topic.slug),
                "duplicate slug: {}",
                topic.slug
            );
            assert!(!topic.slug.contains(char::is_whitespace));
        }
    }

    #[test]
    fn index_rendering_matches_the_default_command() {
        assert_eq!(render(INDEX_SLUG), Some(render_index()));
        assert_eq!(render_index(), README);
        assert_eq!(render("index"), Some(README));
        assert_eq!(
            render("nope"),
            None,
            "unknown slugs must not resolve to content"
        );
    }

    #[test]
    fn list_lines_are_slug_tab_title() {
        let lines = list_lines();
        assert_eq!(lines.len(), 12);
        assert_eq!(lines.len(), topics().len());
        for (line, topic) in lines.iter().zip(TOPICS) {
            let mut fields = line.split('\t');
            assert_eq!(fields.next(), Some(topic.slug));
            assert_eq!(fields.next(), Some(topic.title));
            assert_eq!(fields.next(), None, "line must hold exactly two fields");
        }
    }

    #[test]
    fn json_payloads_are_well_formed() {
        let list = json_list();
        let parsed: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&list).unwrap()).unwrap();
        let topics = parsed["topics"].as_array().expect("topics array");
        assert_eq!(topics.len(), 12);
        assert_eq!(topics[0]["slug"], "index");

        let topic = json_topic(TOPICS[7].slug, TOPICS[7].content);
        let parsed: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&topic).unwrap()).unwrap();
        assert_eq!(parsed["topic"], "tun");
        assert_eq!(parsed["content"].as_str(), Some(TOPICS[7].content));
    }

    /// The rendering path must stay pure: stdout/stderr only, no disk, network,
    /// subprocess or environment access. Scans the non-test part of this file.
    #[test]
    fn manual_rendering_path_has_no_io_side_effects() {
        let source = include_str!("manual.rs");
        let render_path = source.split("#[cfg(test)]").next().expect("source prefix");
        for forbidden in [
            "std::fs",
            "fs::",
            "File::",
            "OpenOptions",
            "std::net",
            "TcpStream",
            "reqwest",
            "Command::new",
            "std::env",
            "tokio",
            "tempfile",
        ] {
            assert!(
                !render_path.contains(forbidden),
                "manual rendering path must not reference `{forbidden}`"
            );
        }
    }
}
