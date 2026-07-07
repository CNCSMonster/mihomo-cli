use crate::mihomo_api;
use dialoguer::FuzzySelect;

/// Flat selector: all groups merged, search by node name.
pub async fn flat_select() -> anyhow::Result<()> {
    let data = mihomo_api::get_all_proxies().await?;
    let nodes = mihomo_api::parse_selector_nodes(&data);
    if nodes.is_empty() {
        anyhow::bail!("No selectable nodes found. Is mihomo running?");
    }

    // Build display labels: [group] node  + ★ if current
    let items: Vec<String> = nodes
        .iter()
        .map(|(g, n, current)| {
            if *current {
                format!("[{}] {}  ★", g, n)
            } else {
                format!("[{}] {}", g, n)
            }
        })
        .collect();

    let selection = FuzzySelect::new()
        .with_prompt("Select node (type to filter, Esc to cancel)")
        .items(&items)
        .default(0)
        .interact_opt()?;

    match selection {
        Some(idx) => {
            let (group, node, _) = &nodes[idx];
            mihomo_api::select_proxy(group, node).await?;
            println!("Switched [{}] → {}", group, node);
            Ok(())
        }
        None => {
            anyhow::bail!("Cancelled.");
        }
    }
}

/// Group-scoped selector (original behaviour, used when --group is specified).
pub async fn select_node(group: &str) -> anyhow::Result<()> {
    let nodes = mihomo_api::get_group_nodes(group).await?;
    if nodes.is_empty() {
        anyhow::bail!("No nodes found in group '{group}'");
    }

    let selection = FuzzySelect::new()
        .with_prompt(format!("Select node for {group} (Esc to cancel)"))
        .items(&nodes)
        .default(0)
        .interact_opt()?;

    match selection {
        Some(idx) => {
            let node = &nodes[idx];
            mihomo_api::select_proxy(group, node).await?;
            println!("Switched {group} → {node}");
            Ok(())
        }
        None => {
            anyhow::bail!("Cancelled.");
        }
    }
}
