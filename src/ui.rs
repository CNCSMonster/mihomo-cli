use crate::mihomo_api;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal;
use std::io::{self, Write};
use std::time::Duration;

pub(crate) fn flat_selector_items(nodes: &[(String, String, bool)]) -> Vec<String> {
    nodes
        .iter()
        .map(|(group, node, current)| flat_selector_item(group, node, *current))
        .collect()
}

pub(crate) fn flat_selector_item(group: &str, node: &str, current: bool) -> String {
    if current {
        format!("[{group}] {node}  ★")
    } else {
        format!("[{group}] {node}")
    }
}

pub(crate) fn selected_flat_node(
    nodes: &[(String, String, bool)],
    selection: Option<usize>,
) -> anyhow::Result<Option<(&str, &str)>> {
    let Some(selection) = selection else {
        return Ok(None);
    };
    nodes
        .get(selection)
        .map(|(group, node, _)| Some((group.as_str(), node.as_str())))
        .ok_or_else(|| anyhow::anyhow!("Invalid selection index: {selection}"))
}

pub(crate) fn selected_group_node(
    nodes: &[String],
    selection: Option<usize>,
) -> anyhow::Result<Option<&str>> {
    let Some(selection) = selection else {
        return Ok(None);
    };
    nodes
        .get(selection)
        .map(|node| Some(node.as_str()))
        .ok_or_else(|| anyhow::anyhow!("Invalid selection index: {selection}"))
}

/// Flat selector: all groups merged, supports j/k navigation + fuzzy filter.
pub(crate) fn flat_select(
    nodes: &[(String, String, bool)],
    selection: Option<usize>,
) -> anyhow::Result<Option<(String, String)>> {
    let Some((group, node)) = selected_flat_node(nodes, selection)? else {
        return Ok(None);
    };
    Ok(Some((group.to_string(), node.to_string())))
}

/// Group-scoped selector result.
pub(crate) fn select_node(
    nodes: &[String],
    selection: Option<usize>,
) -> anyhow::Result<Option<String>> {
    Ok(selected_group_node(nodes, selection)?.map(str::to_string))
}

/// Flat selector: all groups merged, supports j/k navigation + fuzzy filter.
pub(crate) async fn flat_select_with_client(
    client: &impl mihomo_api::MihomoApiClient,
) -> anyhow::Result<Option<(String, String)>> {
    let data = client.get("/proxies").await?;
    let nodes = mihomo_api::parse_selector_nodes(&data);
    if nodes.is_empty() {
        anyhow::bail!("No selectable nodes found. Is mihomo running?");
    }

    let items = flat_selector_items(&nodes);
    let selection = run_selector(
        &items,
        "Select node (↑/↓ or Ctrl+n/p navigate, / filter, Enter select, Esc cancel)",
    )?;
    flat_select(&nodes, selection)
}

/// Group-scoped selector with j/k navigation.
pub(crate) async fn select_node_with_client(
    client: &impl mihomo_api::MihomoApiClient,
    group: &str,
) -> anyhow::Result<Option<String>> {
    let nodes = mihomo_api::get_group_nodes_with_client(client, group).await?;
    if nodes.is_empty() {
        anyhow::bail!("No nodes found in group '{group}'");
    }

    let selection = run_selector(
        &nodes,
        &format!("Select node for {group} (↑/↓ or Ctrl+n/p navigate, Enter select, Esc cancel)"),
    )?;
    select_node(&nodes, selection)
}

/// Interactive selector with j/k navigation, fuzzy filtering, and Enter/Esc.
fn run_selector(items: &[String], prompt: &str) -> anyhow::Result<Option<usize>> {
    terminal::enable_raw_mode()?;
    let result = run_selector_inner(items, prompt);
    terminal::disable_raw_mode()?;
    // Clear screen after exit
    let mut stdout = io::stdout();
    write!(
        stdout,
        "{}{}",
        terminal::Clear(terminal::ClearType::All),
        crossterm::cursor::MoveTo(0, 0)
    )?;
    stdout.flush()?;
    result
}

fn selector_visible_rows(term_height: usize) -> usize {
    term_height.saturating_sub(4).max(1)
}

fn run_selector_inner(items: &[String], prompt: &str) -> anyhow::Result<Option<usize>> {
    let mut cursor: usize = 0;
    let mut filter = String::new();
    let mut in_filter_mode = false;

    loop {
        // Compute filtered indices
        let filtered: Vec<usize> = if filter.is_empty() {
            (0..items.len()).collect()
        } else {
            let lower_filter = filter.to_lowercase();
            items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.to_lowercase().contains(&lower_filter))
                .map(|(i, _)| i)
                .collect()
        };

        if cursor >= filtered.len() && !filtered.is_empty() {
            cursor = filtered.len() - 1;
        }

        // Render
        let mut stdout = io::stdout();
        let term_height = terminal::size().map(|(_, h)| h as usize).unwrap_or(24);
        let visible_rows = selector_visible_rows(term_height); // header + filter line + footer

        write!(
            stdout,
            "{}{}",
            terminal::Clear(terminal::ClearType::All),
            crossterm::cursor::MoveTo(0, 0)
        )?;
        write!(stdout, "  {prompt}\r\n")?;
        if in_filter_mode {
            write!(stdout, "  Filter: {filter}_\r\n")?;
        } else {
            write!(
                stdout,
                "  {}/{}  (press / to filter)\r\n",
                filtered.len(),
                items.len()
            )?;
        }
        write!(stdout, "\r\n")?;

        // Determine scroll window
        let start = if cursor >= visible_rows {
            cursor - visible_rows + 1
        } else {
            0
        };
        let end = (start + visible_rows).min(filtered.len());

        for (display_idx, &real_idx) in filtered.iter().enumerate().take(end).skip(start) {
            let is_current = display_idx == cursor;
            let prefix = if is_current { "▶ " } else { "  " };
            write!(stdout, "{}{}\r\n", prefix, items[real_idx])?;
        }

        write!(stdout, "\r\n")?;
        write!(
            stdout,
            "  j/k: navigate  /: filter  Enter: select  Esc: cancel\r\n"
        )?;
        stdout.flush()?;

        // Wait for key
        let key = loop {
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(k) = event::read()? {
                    break k;
                }
            }
        };

        match key.code {
            KeyCode::Esc if !in_filter_mode => return Ok(None),
            KeyCode::Enter => {
                if filtered.is_empty() {
                    continue;
                }
                return Ok(Some(filtered[cursor]));
            }
            KeyCode::Char('/') => {
                in_filter_mode = true;
                // Don't clear filter — let user append
            }
            KeyCode::Down | KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if cursor + 1 < filtered.len() {
                    cursor += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                cursor = cursor.saturating_sub(1);
            }
            KeyCode::Char('j') if !in_filter_mode => {
                if cursor + 1 < filtered.len() {
                    cursor += 1;
                }
            }
            KeyCode::Char('k') if !in_filter_mode => {
                cursor = cursor.saturating_sub(1);
            }
            KeyCode::Char('g') if !in_filter_mode => {
                cursor = 0;
            }
            KeyCode::Char('G') if !in_filter_mode => {
                if !filtered.is_empty() {
                    cursor = filtered.len() - 1;
                }
            }
            KeyCode::Backspace if in_filter_mode => {
                filter.pop();
                cursor = 0;
            }
            KeyCode::Esc if in_filter_mode => {
                in_filter_mode = false;
                filter.clear();
                cursor = 0;
            }
            KeyCode::Char(c) if in_filter_mode => {
                filter.push(c);
                cursor = 0;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_selector_items_mark_current_node() {
        let nodes = vec![
            ("Proxy".to_string(), "HK 01".to_string(), false),
            ("AI".to_string(), "US GPT".to_string(), true),
        ];

        assert_eq!(
            flat_selector_items(&nodes),
            vec!["[Proxy] HK 01".to_string(), "[AI] US GPT  ★".to_string()]
        );
    }

    #[test]
    fn selector_visible_rows_never_drops_to_zero() {
        assert_eq!(selector_visible_rows(0), 1);
        assert_eq!(selector_visible_rows(3), 1);
        assert_eq!(selector_visible_rows(4), 1);
        assert_eq!(selector_visible_rows(10), 6);
    }

    #[test]
    fn selector_choice_helpers_map_indices_and_cancellation() {
        let nodes = vec![("Proxy".to_string(), "HK 01".to_string(), false)];
        assert_eq!(
            selected_flat_node(&nodes, Some(0)).unwrap(),
            Some(("Proxy", "HK 01"))
        );
        assert_eq!(selected_flat_node(&nodes, None).unwrap(), None);
        assert!(selected_flat_node(&nodes, Some(1))
            .unwrap_err()
            .to_string()
            .contains("Invalid selection index"));

        let group_nodes = vec!["DIRECT".to_string(), "Proxy".to_string()];
        assert_eq!(
            selected_group_node(&group_nodes, Some(1)).unwrap(),
            Some("Proxy")
        );
        assert_eq!(selected_group_node(&group_nodes, None).unwrap(), None);
        assert!(selected_group_node(&group_nodes, Some(2))
            .unwrap_err()
            .to_string()
            .contains("Invalid selection index"));
    }

    #[test]
    fn selector_results_are_owned_for_command_layer_submission() {
        let nodes = vec![("Proxy".to_string(), "HK 01".to_string(), false)];
        assert_eq!(
            flat_select(&nodes, Some(0)).unwrap(),
            Some(("Proxy".to_string(), "HK 01".to_string()))
        );
        assert_eq!(flat_select(&nodes, None).unwrap(), None);

        let group_nodes = vec!["DIRECT".to_string(), "Proxy".to_string()];
        assert_eq!(
            select_node(&group_nodes, Some(1)).unwrap(),
            Some("Proxy".to_string())
        );
        assert_eq!(select_node(&group_nodes, None).unwrap(), None);
    }
}
