//! Resolve only the visible tree node's configured tokens, retaining the shared
//! conditional styling rules without rebuilding the complete agent panel.
use super::*;
use ratatui::{
    text::Line,
    widgets::{Paragraph, Widget},
};

pub(super) fn render_row_content(
    buffer: &mut Buffer,
    rect: Rect,
    row: &ClientNavigatorRow,
    endpoints: &[ClientShellEndpoint],
    config: &ClientShellConfig,
    highlighted: bool,
) {
    if let ClientNavigatorTarget::Machine { endpoint_id } = &row.target {
        if let Some(endpoint) = endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
        {
            let palette = &config.palette;
            let (glyph, status, color) = endpoint_status_presentation(endpoint.status, palette);
            let signal = if endpoint_id.is_local() {
                String::new()
            } else if endpoint.status == ClientEndpointStatus::Online {
                glyph.to_owned()
            } else {
                format!("{glyph} {status}")
            };
            let signal_width = render::display_width(&signal).min(rect.width);
            render::put_text(
                buffer,
                rect.x,
                rect.y,
                rect.width.saturating_sub(signal_width.saturating_add(1)),
                &row.label,
                Style::default()
                    .fg(if row.stale {
                        palette.overlay0
                    } else {
                        palette.text
                    })
                    .add_modifier(Modifier::BOLD),
            );
            render::put_right_text(buffer, rect, rect.y, &signal, Style::default().fg(color));
            if highlighted && !row.stale {
                buffer.set_style(
                    rect,
                    Style::default()
                        .fg(palette.panel_bg)
                        .bg(palette.accent)
                        .add_modifier(Modifier::BOLD),
                );
            }
            return;
        }
    }
    let tokens = row_tokens(row, endpoints, config);
    let palette = &config.palette;
    let status = row
        .status
        .unwrap_or(crate::api::schema::AgentStatus::Unknown);
    let primary = Style::default()
        .fg(palette.text)
        .add_modifier(Modifier::BOLD);
    let secondary = Style::default().fg(palette.overlay0);
    let status_style = Style::default().fg(status_color(status, palette));
    let spans = crate::ui::resolved_token_spans(
        &tokens,
        (status_icon(status, config.status_indicators), status_style),
        status_style,
        primary,
        secondary,
        secondary,
        palette,
        rect.width as usize,
    );
    Paragraph::new(Line::from(spans)).render(rect, buffer);
    // Selection must remain readable even when a token has an explicit color.
    if row.stale {
        buffer.set_style(
            rect,
            Style::default()
                .fg(palette.overlay0)
                .add_modifier(Modifier::DIM),
        );
    } else if highlighted {
        buffer.set_style(
            rect,
            Style::default()
                .fg(palette.panel_bg)
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD)
                .remove_modifier(Modifier::DIM),
        );
    }
}

fn row_tokens(
    row: &ClientNavigatorRow,
    endpoints: &[ClientShellEndpoint],
    config: &ClientShellConfig,
) -> Vec<crate::ui::ResolvedToken> {
    let fallback = || {
        vec![crate::ui::ResolvedToken {
            kind: crate::ui::ResolvedTokenKind::Pane(row.label.clone()),
            style: Default::default(),
        }]
    };
    let endpoint_id = match &row.target {
        ClientNavigatorTarget::Machine { .. } | ClientNavigatorTarget::Tab { .. } => {
            return fallback()
        }
        ClientNavigatorTarget::Workspace { endpoint_id, .. }
        | ClientNavigatorTarget::Pane { endpoint_id, .. } => endpoint_id,
    };
    let Some(endpoint) = endpoints
        .iter()
        .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
    else {
        return fallback();
    };
    let Some(snapshot) = endpoint.snapshot.as_deref() else {
        return fallback();
    };
    match &row.target {
        ClientNavigatorTarget::Workspace { workspace_id, .. } => {
            let Some(workspace) = snapshot
                .workspaces
                .iter()
                .find(|workspace| &workspace.workspace_id == workspace_id)
            else {
                return fallback();
            };
            super::sidebar::workspace_rows(
                workspace,
                row.status.unwrap_or(workspace.agent_status),
                false,
                &config.spaces,
            )
            .into_iter()
            .flatten()
            .collect()
        }
        ClientNavigatorTarget::Pane { pane_id, .. } => {
            let Some(agent) = snapshot
                .agents
                .iter()
                .find(|agent| &agent.pane_id == pane_id)
            else {
                return fallback();
            };
            let Some(workspace) = snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == agent.workspace_id)
            else {
                return fallback();
            };
            let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == agent.tab_id);
            let pane = snapshot.panes.iter().find(|pane| &pane.pane_id == pane_id);
            let tab_count = snapshot
                .tabs
                .iter()
                .filter(|tab| tab.workspace_id == agent.workspace_id)
                .count();
            let tokens = agent.tokens.iter().cloned().collect::<HashMap<_, _>>();
            let state_text = agent
                .state_labels
                .iter()
                .find(|(key, _)| key == status_text(agent.agent_status))
                .map(|(_, value)| value.as_str())
                .unwrap_or_else(|| match agent.agent_status {
                    crate::api::schema::AgentStatus::Unknown => "idle",
                    status => status_text(status),
                });
            crate::ui::sidebar_agent_rows(
                &config.agents,
                crate::ui::AgentTokenContext {
                    machine: Some(&endpoint.label),
                    workspace: &workspace.label,
                    tab: tab
                        .filter(|tab| tab_count > 1 || tab.custom_label)
                        .map(|tab| tab.label.as_str()),
                    pane: agent
                        .title
                        .as_deref()
                        .or_else(|| pane.and_then(|pane| pane.label.as_deref())),
                    agent_label: agent
                        .display_agent
                        .as_deref()
                        .or(agent.name.as_deref())
                        .or(agent.agent.as_deref())
                        .or(agent.title.as_deref()),
                    terminal_title: agent.terminal_title.as_deref(),
                    terminal_title_stripped: agent.terminal_title_stripped.as_deref(),
                    canonical_agent: agent
                        .agent
                        .as_deref()
                        .and_then(crate::detect::parse_agent_label),
                    tokens: &tokens,
                },
                state_text,
            )
            .into_iter()
            .flatten()
            .collect()
        }
        _ => fallback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn workspace_tree_tokens_flatten_rows_and_preserve_conditional_styles() {
        let config: Config = toml::from_str(
            r##"
[ui.sidebar.spaces]
rows = [["workspace"], [{ token = "$load", rules = [{ gt = 80, fg = "#ff0000" }] }]]
"##,
        )
        .expect("valid token rules");
        let config = ClientShellConfig::from_config(&config);
        let mut snapshot = super::super::tests::snapshot();
        snapshot.workspaces[0]
            .tokens
            .push(("load".into(), "90".into()));
        let mut endpoint = super::super::local_endpoint();
        endpoint.snapshot = Some(snapshot.into());
        let row = ClientNavigatorRow {
            depth: 0,
            label: "client-shell".into(),
            meta: String::new(),
            status: Some(crate::api::schema::AgentStatus::Idle),
            stale: false,
            current: false,
            target: ClientNavigatorTarget::Workspace {
                endpoint_id: ClientEndpointId::Local,
                workspace_id: "ws_1".into(),
            },
        };
        let area = Rect::new(0, 0, 50, 1);
        let mut buffer = Buffer::empty(area);
        render_row_content(&mut buffer, area, &row, &[endpoint], &config, false);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("client-shell · 90"), "{text}");
        let load = text.find("90").expect("load token is visible");
        // All text before the load is ASCII except the single-cell separator.
        let x = unicode_width::UnicodeWidthStr::width(&text[..load]) as u16;
        assert_eq!(buffer[(x, 0)].fg, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn focused_tree_row_overrides_token_colors_for_accent_contrast() {
        let config = ClientShellConfig::from_config(&Config::default());
        let row = ClientNavigatorRow {
            depth: 0,
            label: "shell".into(),
            meta: String::new(),
            status: None,
            stale: false,
            current: true,
            target: ClientNavigatorTarget::Pane {
                endpoint_id: ClientEndpointId::Local,
                pane_id: "pane_1".into(),
            },
        };
        let area = Rect::new(0, 0, 20, 1);
        let mut buffer = Buffer::empty(area);
        render_row_content(&mut buffer, area, &row, &[], &config, true);
        assert_eq!(
            (buffer[(0, 0)].fg, buffer[(0, 0)].bg),
            (config.palette.panel_bg, config.palette.accent)
        );
    }
    #[test]
    #[ignore = "manual client sidebar scaling profile"]
    fn sidebar_tree_render_scale_profile() {
        for count in [1, 15] {
            let mut snapshot = super::super::tests::snapshot();
            let template = snapshot.panes[0].clone();
            snapshot.panes = (0..count)
                .map(|index| {
                    let mut pane = template.clone();
                    pane.pane_id = format!("pane_{}", index + 1);
                    pane.label = Some(format!("terminal {}", index + 1));
                    pane.focused = index == 0;
                    pane
                })
                .collect();
            snapshot.agents = snapshot
                .panes
                .iter()
                .map(|pane| crate::protocol::ClientShellAgent {
                    pane_id: pane.pane_id.clone(),
                    workspace_id: pane.workspace_id.clone(),
                    tab_id: pane.tab_id.clone(),
                    name: Some("coding agent".into()),
                    display_agent: None,
                    agent: Some("pi".into()),
                    title: Some("working on task".into()),
                    terminal_title: Some("task".into()),
                    terminal_title_stripped: Some("task".into()),
                    agent_status: crate::api::schema::AgentStatus::Working,
                    state_change_seq: 1,
                    state_labels: vec![("working".into(), "working".into())],
                    tokens: vec![("summary".into(), "running checks".into())],
                    focused: pane.focused,
                })
                .collect();
            let mut state =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            state.set_snapshot(Box::new(snapshot));
            for _ in 0..50 {
                std::hint::black_box(state.compose(120, 40));
            }
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                std::hint::black_box(state.compose(120, 40));
            }
            eprintln!(
                "client sidebar: panes={count} mean_us={:.2}",
                start.elapsed().as_micros() as f64 / 1000.0
            );
        }
    }
}
