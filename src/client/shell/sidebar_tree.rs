//! Client-owned workspace/tab/pane navigation. Targets are qualified by endpoint,
//! so cached remote IDs can never accidentally focus an identically named local pane.
use super::*;
use crossterm::event::KeyModifiers;

#[derive(Default)]
pub(super) struct SidebarTree {
    pub(super) selected: Option<ClientNavigatorTarget>,
    collapsed: Vec<ClientNavigatorTarget>,
    query: String,
    searching: bool,
    jump: Option<String>,
    scroll: usize,
}

fn rows(
    endpoints: &[ClientShellEndpoint],
    active: &ClientEndpointId,
    tree: &SidebarTree,
    navigating: bool,
    collapsed_groups: &HashSet<String>,
    collapsed_endpoints: &HashSet<ClientEndpointId>,
) -> Vec<ClientNavigatorRow> {
    let mut source = Vec::new();
    let federated = endpoints.len() > 1;
    for endpoint in endpoints {
        let stale = endpoint.status != ClientEndpointStatus::Online;
        let endpoint_id = &endpoint.endpoint_id;
        if federated {
            source.push(ClientNavigatorRow {
                sole_pane_id: None,
                depth: 0,
                label: endpoint.label.clone(),
                meta: String::new(),
                status: None,
                stale,
                current: false,
                target: ClientNavigatorTarget::Machine {
                    endpoint_id: endpoint_id.clone(),
                },
            });
        }
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            continue;
        };
        // Index each snapshot once. Avoid scanning every pane for every tab on
        // this per-frame path, which is multiplied by attached clients.
        let mut tabs = HashMap::<&str, Vec<&ClientShellTab>>::new();
        for tab in &snapshot.tabs {
            tabs.entry(&tab.workspace_id).or_default().push(tab);
        }
        let mut panes = HashMap::<&str, Vec<&crate::protocol::ClientShellPane>>::new();
        for pane in &snapshot.panes {
            panes.entry(&pane.tab_id).or_default().push(pane);
        }
        let agents = snapshot
            .agents
            .iter()
            .map(|agent| (agent.pane_id.as_str(), agent))
            .collect::<HashMap<_, _>>();
        let expanded_groups = HashSet::new();
        let groups = if navigating && !tree.query.is_empty() || endpoint_id != active {
            &expanded_groups
        } else {
            collapsed_groups
        };
        for entry in sidebar::workspace_entries(snapshot, groups) {
            let workspace = &snapshot.workspaces[entry.index];
            let depth = u8::from(federated) + u8::from(entry.indented);
            let workspace_tabs = tabs
                .get(workspace.workspace_id.as_str())
                .map_or(&[][..], Vec::as_slice);
            let sole_pane = if let [tab] = workspace_tabs {
                panes.get(tab.tab_id.as_str()).and_then(|panes| {
                    if let [pane] = panes.as_slice() {
                        Some(*pane)
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            let sole_agent = sole_pane.and_then(|pane| agents.get(pane.pane_id.as_str()).copied());
            source.push(ClientNavigatorRow {
                sole_pane_id: sole_pane.map(|pane| pane.pane_id.clone()),
                depth,
                label: workspace.label.clone(),
                meta: sole_pane.map_or_else(
                    || workspace.branch.clone().unwrap_or_default(),
                    |pane| {
                        format!(
                            "{} {} {}",
                            workspace.branch.as_deref().unwrap_or_default(),
                            pane.label
                                .as_deref()
                                .or_else(|| sole_agent.and_then(|agent| agent.title.as_deref()))
                                .unwrap_or("pane 1"),
                            pane.foreground_cwd
                                .as_deref()
                                .or(pane.cwd.as_deref())
                                .unwrap_or_default()
                        )
                    },
                ),
                status: Some(sidebar::displayed_workspace_status(
                    snapshot, workspace, groups,
                )),
                stale,
                current: endpoint_id == active
                    && snapshot.focused_workspace_id.as_deref() == Some(&workspace.workspace_id)
                    && sole_pane.is_some_and(|pane| {
                        snapshot.focused_pane_id.as_deref() == Some(&pane.pane_id)
                    }),
                target: ClientNavigatorTarget::Workspace {
                    endpoint_id: endpoint_id.clone(),
                    workspace_id: workspace.workspace_id.clone(),
                },
            });
            if sole_pane.is_some() {
                continue;
            }
            for tab in workspace_tabs {
                let tab_panes = panes
                    .get(tab.tab_id.as_str())
                    .map_or(&[][..], Vec::as_slice);
                let show_tab = workspace_tabs.len() > 1 && tab_panes.len() != 1;
                if show_tab {
                    source.push(ClientNavigatorRow {
                        sole_pane_id: None,
                        depth: depth + 1,
                        label: tab.label.clone(),
                        meta: format!("{} panes", tab_panes.len()),
                        status: Some(tab.agent_status),
                        stale,
                        current: false,
                        target: ClientNavigatorTarget::Tab {
                            endpoint_id: endpoint_id.clone(),
                            tab_id: tab.tab_id.clone(),
                        },
                    });
                }
                for (index, pane) in tab_panes.iter().enumerate() {
                    let agent = agents.get(pane.pane_id.as_str()).copied();
                    let label = pane
                        .label
                        .clone()
                        .or_else(|| agent.and_then(|agent| agent.title.clone()))
                        .unwrap_or_else(|| format!("pane {}", index + 1));
                    source.push(ClientNavigatorRow {
                        sole_pane_id: None,
                        depth: depth + 1 + u8::from(show_tab),
                        label,
                        meta: pane
                            .foreground_cwd
                            .clone()
                            .or_else(|| pane.cwd.clone())
                            .unwrap_or_default(),
                        status: Some(
                            agent.map_or(crate::api::schema::AgentStatus::Unknown, |agent| {
                                agent.agent_status
                            }),
                        ),
                        stale,
                        current: endpoint_id == active
                            && snapshot.focused_pane_id.as_deref() == Some(&pane.pane_id),
                        target: ClientNavigatorTarget::Pane {
                            endpoint_id: endpoint_id.clone(),
                            pane_id: pane.pane_id.clone(),
                        },
                    });
                }
            }
        }
    }
    let query = if navigating {
        tree.query.trim().to_lowercase()
    } else {
        String::new()
    };
    if !query.is_empty() {
        // Keep matching parents' descendants and matching children's ancestors.
        // A workspace or machine search therefore exposes its whole subtree.
        let mut keep = vec![false; source.len()];
        let mut ancestors = Vec::<usize>::new();
        let mut matched_depth = None;
        for (index, row) in source.iter().enumerate() {
            while ancestors
                .last()
                .is_some_and(|ancestor| source[*ancestor].depth >= row.depth)
            {
                ancestors.pop();
            }
            if matched_depth.is_some_and(|depth| row.depth <= depth) {
                matched_depth = None;
            }
            let search = format!(
                "{} {} {}",
                row.label,
                row.meta,
                row.status.map(status_text).unwrap_or_default()
            )
            .to_lowercase();
            let matched = query
                .split_whitespace()
                .all(|needle| search.contains(needle));
            if matched || matched_depth.is_some() {
                keep[index] = true;
                for ancestor in &ancestors {
                    keep[*ancestor] = true;
                }
                if matched && matched_depth.is_none() {
                    matched_depth = Some(row.depth);
                }
            }
            ancestors.push(index);
        }
        return source
            .into_iter()
            .zip(keep)
            .filter_map(|(row, keep)| keep.then_some(row))
            .collect();
    }
    let mut hidden_depth = None;
    source
        .into_iter()
        .filter(|row| {
            if hidden_depth.is_some_and(|depth| row.depth > depth) {
                return false;
            }
            hidden_depth = None;
            if tree.collapsed.contains(&row.target)
                || matches!(&row.target, ClientNavigatorTarget::Machine { endpoint_id } if collapsed_endpoints.contains(endpoint_id))
            {
                hidden_depth = Some(row.depth);
            }
            true
        })
        .collect()
}

pub(super) fn render_tree(
    buffer: &mut Buffer,
    area: Rect,
    config: &ClientShellConfig,
    state: &mut render::ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    render::render_sidebar_background(buffer, area, palette);
    if area.width < 2 || area.height == 0 {
        return;
    }
    hits.sidebar_divider = Rect::new(area.right() - 1, area.y, 1, area.height);
    let content = Rect::new(area.x, area.y, area.width - 1, area.height);
    let body = Rect::new(
        content.x,
        content.y.saturating_add(1),
        content.width,
        content.height.saturating_sub(2),
    );
    hits.workspace_body = body;
    let rows = rows(
        state.endpoints,
        state.active_endpoint_id,
        state.sidebar_tree,
        state.navigating,
        state.collapsed_groups,
        state.collapsed_endpoints,
    );
    let focused_snapshot = state
        .endpoints
        .iter()
        .find(|endpoint| &endpoint.endpoint_id == state.active_endpoint_id)
        .and_then(|endpoint| endpoint.snapshot.as_deref());
    let focused_pane_in_workspace = focused_snapshot.is_some_and(|snapshot| {
        snapshot.panes.iter().any(|pane| {
            Some(pane.pane_id.as_str()) == snapshot.focused_pane_id.as_deref()
                && Some(pane.workspace_id.as_str()) == snapshot.focused_workspace_id.as_deref()
        })
    });
    let tree = &mut state.sidebar_tree;
    tree.scroll = *state.workspace_scroll;
    if state.navigating
        && !rows
            .iter()
            .any(|row| Some(&row.target) == tree.selected.as_ref())
    {
        tree.selected = rows
            .iter()
            .find(|row| row.current)
            .or_else(|| rows.first())
            .map(|row| row.target.clone());
    }
    if !body.is_empty() && (state.navigating || std::mem::take(state.reveal_focused_workspace)) {
        if let Some(index) = rows.iter().position(|row| {
            if state.navigating {
                Some(&row.target) == tree.selected.as_ref()
            } else if focused_pane_in_workspace {
                row.current
            } else {
                matches!(&row.target, ClientNavigatorTarget::Workspace { endpoint_id, workspace_id } if endpoint_id == state.active_endpoint_id && focused_snapshot.and_then(|snapshot| snapshot.focused_workspace_id.as_deref()) == Some(workspace_id.as_str()))
            }
        }) {
            if index < tree.scroll {
                tree.scroll = index;
            }
            if index >= tree.scroll + body.height as usize {
                tree.scroll = index.saturating_sub(body.height.saturating_sub(1) as usize);
            }
        }
    }
    tree.scroll = tree
        .scroll
        .min(rows.len().saturating_sub(body.height as usize));
    // Use the shared wheel/scrollbar path, with one unit per tree row.
    *state.workspace_scroll = tree.scroll;
    let metrics = super::scroll::list_scroll_metrics(
        &vec![1; rows.len()],
        &vec![0; rows.len()],
        body.height,
        tree.scroll,
    );
    hits.workspace_max_scroll = metrics.max_offset_from_bottom;
    hits.workspace_scroll_metrics = Some(metrics);
    let title = if let Some(number) = &tree.jump {
        format!(" jump: {number} · enter")
    } else if tree.searching || !tree.query.is_empty() {
        format!(" /{}", tree.query)
    } else {
        " spaces".to_owned()
    };
    render::put_text(
        buffer,
        content.x,
        content.y,
        content.width,
        &title,
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
    );
    let mut number = rows
        .iter()
        .take(tree.scroll)
        .filter(|row| row.represents_pane())
        .count();
    for (offset, row) in rows
        .iter()
        .skip(tree.scroll)
        .take(body.height as usize)
        .enumerate()
    {
        let rect = Rect::new(body.x, body.y + offset as u16, body.width, 1);
        let pane = row.represents_pane();
        let gutter = if pane {
            number += 1;
            format!("{number:>2} ")
        } else {
            "   ".to_owned()
        };
        let marker = if pane {
            "•"
        } else if tree.collapsed.contains(&row.target)
            || matches!(&row.target, ClientNavigatorTarget::Machine { endpoint_id } if state.collapsed_endpoints.contains(endpoint_id))
        {
            "▸"
        } else {
            "▾"
        };
        let indent = if row.depth > 0 {
            format!("{}└─", "  ".repeat(row.depth as usize - 1))
        } else {
            String::new()
        };
        let label = format!("{gutter}{indent}{marker} ");
        let selected = if state.navigating {
            Some(&row.target) == tree.selected.as_ref()
        } else {
            row.current
        };
        let style = if selected && !row.stale {
            Style::default()
                .fg(palette.panel_bg)
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else if row.stale {
            Style::default()
                .fg(palette.overlay0)
                .add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(palette.text)
        };
        buffer.set_style(rect, style);
        render::put_text(buffer, rect.x, rect.y, rect.width, &label, style);
        let prefix_width = UnicodeWidthStr::width(label.as_str()).min(rect.width as usize) as u16;
        super::sidebar_tree_tokens::render_row_content(
            buffer,
            Rect::new(rect.x + prefix_width, rect.y, rect.width - prefix_width, 1),
            row,
            state.endpoints,
            config,
            selected && !row.stale,
        );
        if let ClientNavigatorTarget::Workspace {
            endpoint_id,
            workspace_id,
        } = &row.target
        {
            let group_toggle = state
                .endpoints
                .iter()
                .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
                .and_then(|endpoint| endpoint.snapshot.as_deref())
                .and_then(|snapshot| {
                    snapshot
                        .workspaces
                        .iter()
                        .position(|workspace| &workspace.workspace_id == workspace_id)
                        .and_then(|index| sidebar::parent_group_key(snapshot, index))
                })
                .filter(|_| endpoint_id == state.active_endpoint_id)
                .map(|key| {
                    let toggle = Rect::new(rect.right().saturating_sub(1), rect.y, 1, 1);
                    render::put_text(
                        buffer,
                        toggle.x,
                        toggle.y,
                        1,
                        if state.collapsed_groups.contains(&key) {
                            "▸"
                        } else {
                            "▾"
                        },
                        Style::default().fg(palette.accent),
                    );
                    (toggle, key)
                });
            hits.workspaces.push(WorkspaceHit {
                rect,
                endpoint_id: endpoint_id.clone(),
                workspace_id: workspace_id.clone(),
                indented: row.depth > u8::from(state.endpoints.len() > 1),
                group_toggle,
            });
        }
        if let ClientNavigatorTarget::Machine { endpoint_id } = &row.target {
            hits.machines.push(MachineHit {
                rect,
                endpoint_id: endpoint_id.clone(),
            });
        }
        hits.sidebar_tree_rows.push((rect, row.target.clone()));
    }
    if metrics.max_offset_from_bottom > 0 && body.width > 1 {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.workspace_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, palette);
    }
    if let Some(y) = state
        .workspace_drop_indicator_row
        .filter(|y| *y >= body.y && *y < body.bottom())
    {
        render::put_text(
            buffer,
            body.x,
            y,
            body.width,
            &"─".repeat(body.width as usize),
            Style::default().fg(palette.accent),
        );
    }
    let footer = content.bottom().saturating_sub(1);
    if config.mouse_capture {
        hits.new_workspace = Rect::new(content.x, footer, 5.min(content.width), 1);
        render::put_text(
            buffer,
            content.x,
            footer,
            content.width,
            " new",
            Style::default().fg(palette.overlay0),
        );
        let attention = state
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == state.active_endpoint_id)
            .and_then(|endpoint| endpoint.snapshot.as_deref())
            .is_some_and(super::global_menu::global_menu_attention);
        let launcher_width = if attention { 8 } else { 6 }.min(content.width);
        hits.global_launcher = Rect::new(
            content.right().saturating_sub(launcher_width + 1),
            footer,
            launcher_width,
            1,
        );
        render::put_right_text(
            buffer,
            hits.global_launcher,
            footer,
            "menu",
            Style::default().fg(palette.overlay0),
        );
        if attention {
            render::put_text(
                buffer,
                hits.global_launcher.right().saturating_sub(6),
                footer,
                1,
                "●",
                Style::default().fg(palette.accent),
            );
        }
    }
    hits.sidebar_toggle = Rect::new(content.right().saturating_sub(1), footer, 1, 1);
    render::put_text(
        buffer,
        hits.sidebar_toggle.x,
        footer,
        1,
        "«",
        Style::default().fg(palette.overlay0),
    );
}

impl ClientShellState {
    pub(super) fn reset_sidebar_search(&mut self) {
        self.sidebar_tree.query.clear();
        self.sidebar_tree.searching = false;
        self.sidebar_tree.jump = None;
    }

    pub(super) fn sidebar_prompt_active(&self) -> bool {
        self.sidebar_tree.jump.is_some()
            || self.mode == ClientShellMode::Navigate && self.sidebar_tree.searching
    }

    pub(super) fn insert_sidebar_text(&mut self, text: &str) -> bool {
        if self.overlay.is_some() {
            return false;
        }
        if let Some(input) = &mut self.sidebar_tree.jump {
            input.extend(
                text.chars()
                    .filter(char::is_ascii_digit)
                    .take(9usize.saturating_sub(input.len())),
            );
            return true;
        }
        if self.mode == ClientShellMode::Navigate && self.sidebar_tree.searching {
            self.sidebar_tree
                .query
                .extend(text.chars().filter(|c| !c.is_control()));
            self.sidebar_tree.scroll = 0;
            self.workspace_scroll = 0;
            return true;
        }
        false
    }

    pub(super) fn move_sidebar_selection(&mut self, delta: isize) {
        let rows = self.sidebar_rows();
        if rows.is_empty() {
            return;
        }
        let current = rows
            .iter()
            .position(|row| Some(&row.target) == self.sidebar_tree.selected.as_ref())
            .or_else(|| rows.iter().position(|row| row.current))
            .unwrap_or(0);
        let next = current.saturating_add_signed(delta).min(rows.len() - 1);
        self.sidebar_tree.selected = Some(rows[next].target.clone());
    }

    pub(super) fn prepare_sidebar_workspace_action(
        &mut self,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.mobile_layout_active() {
            return true;
        }
        let Some(target) = self.sidebar_tree.selected.as_ref() else {
            return false;
        };
        if !self.sidebar_rows().iter().any(|row| &row.target == target) {
            return false;
        }
        let (endpoint_id, workspace_id) = match target {
            ClientNavigatorTarget::Machine { endpoint_id } => (endpoint_id, None),
            ClientNavigatorTarget::Workspace {
                endpoint_id,
                workspace_id,
            } => (endpoint_id, Some(workspace_id.clone())),
            ClientNavigatorTarget::Tab {
                endpoint_id,
                tab_id,
            } => (
                endpoint_id,
                self.snapshot
                    .as_deref()
                    .and_then(|snapshot| snapshot.tabs.iter().find(|tab| &tab.tab_id == tab_id))
                    .map(|tab| tab.workspace_id.clone()),
            ),
            ClientNavigatorTarget::Pane {
                endpoint_id,
                pane_id,
            } => (
                endpoint_id,
                self.snapshot
                    .as_deref()
                    .and_then(|snapshot| {
                        snapshot.panes.iter().find(|pane| &pane.pane_id == pane_id)
                    })
                    .map(|pane| pane.workspace_id.clone()),
            ),
        };
        if endpoint_id != &self.active_endpoint_id {
            self.receive_endpoint_unavailable(
                "Activate this machine before changing its workspaces".to_owned(),
            );
            outcome.repaint = true;
            return false;
        }
        if workspace_id.as_ref().is_none_or(|id| {
            !self.snapshot.as_deref().is_some_and(|snapshot| {
                snapshot
                    .workspaces
                    .iter()
                    .any(|workspace| &workspace.workspace_id == id)
            })
        }) {
            return false;
        }
        self.navigate_workspace_id = workspace_id;
        true
    }

    fn sidebar_rows(&self) -> Vec<ClientNavigatorRow> {
        rows(
            &self.endpoints,
            &self.active_endpoint_id,
            &self.sidebar_tree,
            self.mode == ClientShellMode::Navigate,
            &self.collapsed_groups,
            &self.collapsed_endpoints,
        )
    }

    fn focus_sidebar_target(
        &mut self,
        target: ClientNavigatorTarget,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let target = if matches!(target, ClientNavigatorTarget::Workspace { .. }) {
            self.sidebar_rows()
                .iter()
                .find(|row| row.target == target)
                .and_then(ClientNavigatorRow::sole_pane_target)
                .unwrap_or(target)
        } else {
            target
        };
        let focused = match target {
            ClientNavigatorTarget::Machine { endpoint_id } => {
                self.activate_endpoint(endpoint_id, outcome)
            }
            ClientNavigatorTarget::Workspace {
                endpoint_id,
                workspace_id,
            } => self.focus_or_activate(
                endpoint_id,
                ClientEndpointFocusTarget::Workspace(workspace_id),
                outcome,
            ),
            ClientNavigatorTarget::Tab {
                endpoint_id,
                tab_id,
            } => {
                self.focus_or_activate(endpoint_id, ClientEndpointFocusTarget::Tab(tab_id), outcome)
            }
            ClientNavigatorTarget::Pane {
                endpoint_id,
                pane_id,
            } => self.focus_or_activate(
                endpoint_id,
                ClientEndpointFocusTarget::Pane(pane_id),
                outcome,
            ),
        };
        if focused {
            self.sidebar_tree.query.clear();
            self.sidebar_tree.searching = false;
            self.mode = ClientShellMode::Terminal;
        }
        outcome.repaint = true;
        focused
    }

    fn jump_sidebar(&mut self, index: usize, outcome: &mut ClientShellInput) -> bool {
        if let Some(row) = self
            .sidebar_rows()
            .into_iter()
            .filter(|row| row.represents_pane())
            .nth(index)
        {
            self.sidebar_tree.selected = Some(row.target.clone());
            let focused = self.focus_sidebar_target(row.target, outcome);
            self.reveal_focused_workspace |= focused;
            return focused;
        }
        false
    }

    pub(super) fn handle_sidebar_action(
        &mut self,
        action: crate::input::KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        match action {
            crate::input::KeybindAction::JumpSidebarItem(index) => {
                if !self.mobile_layout_active() && !self.sidebar_collapsed {
                    self.jump_sidebar(index, outcome);
                }
            }
            crate::input::KeybindAction::JumpSidebarItemPrompt => {
                if self.mobile_layout_active() {
                    return true;
                }
                self.mode = ClientShellMode::Navigate;
                self.sidebar_tree.searching = false;
                self.sidebar_tree.jump = Some(String::new());
                self.sidebar_collapsed = false;
                outcome.repaint = true;
                outcome.resize = true;
            }
            _ => return false,
        }
        true
    }

    pub(super) fn route_sidebar_prompt(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if let Some(input) = &mut self.sidebar_tree.jump {
            match key.code {
                KeyCode::Esc => {
                    self.sidebar_tree.jump = None;
                }
                KeyCode::Enter => {
                    let number = input.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
                    if let Some(index) = number {
                        if self.jump_sidebar(index, outcome) {
                            self.sidebar_tree.jump = None;
                        }
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if c.is_ascii_digit() && input.len() < 9 => input.push(c),
                _ => {}
            }
            outcome.repaint = true;
            return true;
        }
        if self.mode == ClientShellMode::Navigate && self.sidebar_tree.searching {
            if let Some(crate::input::KeybindMatch::Action(action)) =
                crate::input::resolve_direct_binding(&self.config.keybinds.keybinds, key)
            {
                if self.handle_sidebar_action(action, outcome) {
                    return true;
                }
            }
            match key.code {
                KeyCode::Esc => {
                    self.sidebar_tree.searching = false;
                }
                KeyCode::Enter => {
                    let rows = self.sidebar_rows();
                    if let Some(row) = rows
                        .iter()
                        .find(|row| Some(&row.target) == self.sidebar_tree.selected.as_ref())
                        .or_else(|| rows.first())
                    {
                        self.focus_sidebar_target(row.target.clone(), outcome);
                    }
                    self.sidebar_tree.searching = false;
                }
                KeyCode::Up => self.move_sidebar_selection(-1),
                KeyCode::Down => self.move_sidebar_selection(1),
                KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
                    self.move_sidebar_selection(-1)
                }
                KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
                    self.move_sidebar_selection(1)
                }
                KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                    self.sidebar_tree.query.clear()
                }
                KeyCode::Backspace => {
                    self.sidebar_tree.query.pop();
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.sidebar_tree.query.push(c)
                }
                _ => return false,
            }
            self.sidebar_tree.scroll = 0;
            outcome.repaint = true;
            return true;
        }
        false
    }

    pub(super) fn route_sidebar_navigation(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.mobile_layout_active() {
            return false;
        }
        if let Some(crate::input::KeybindMatch::Action(action)) =
            crate::input::resolve_direct_binding(&self.config.keybinds.keybinds, key)
        {
            if self.handle_sidebar_action(action, outcome) {
                return true;
            }
        }
        let rows = self.sidebar_rows();
        let current = rows
            .iter()
            .position(|row| Some(&row.target) == self.sidebar_tree.selected.as_ref())
            .or_else(|| rows.iter().position(|row| row.current))
            .unwrap_or(0);
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        let up = self
            .config
            .keybinds
            .keybinds
            .navigate
            .workspace_up
            .matches_direct_key(key)
            || self
                .config
                .keybinds
                .keybinds
                .navigate
                .pane_up
                .matches_direct_key(key);
        let down = self
            .config
            .keybinds
            .keybinds
            .navigate
            .workspace_down
            .matches_direct_key(key)
            || self
                .config
                .keybinds
                .keybinds
                .navigate
                .pane_down
                .matches_direct_key(key);
        let left = self
            .config
            .keybinds
            .keybinds
            .navigate
            .pane_left
            .matches_direct_key(key);
        let right = self
            .config
            .keybinds
            .keybinds
            .navigate
            .pane_right
            .matches_direct_key(key);
        let code = if left {
            KeyCode::Left
        } else if right {
            KeyCode::Right
        } else {
            code
        };
        let modifiers = if left || right {
            KeyModifiers::empty()
        } else {
            modifiers
        };
        if !up
            && !down
            && !left
            && !right
            && (crate::input::resolve_prefix_binding(&self.config.keybinds.keybinds, key).is_some())
        {
            return false;
        }
        if up || down {
            if !rows.is_empty() {
                let index = if up {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(rows.len() - 1)
                };
                self.sidebar_tree.selected = Some(rows[index].target.clone());
            }
        } else if modifiers.is_empty() {
            match code {
                KeyCode::Home => {
                    self.sidebar_tree.selected = rows.first().map(|row| row.target.clone());
                }
                KeyCode::End => {
                    self.sidebar_tree.selected = rows.last().map(|row| row.target.clone());
                }
                KeyCode::Backspace => self.sidebar_tree.query.clear(),
                KeyCode::Char(' ') => {
                    if let Some(row) = rows.get(current).filter(|row| !row.represents_pane()) {
                        self.toggle_sidebar_target(row.target.clone());
                    }
                }
                KeyCode::Char('/') => {
                    self.sidebar_tree.searching = true;
                }
                KeyCode::Esc => {
                    self.sidebar_tree.query.clear();
                    self.sidebar_tree.selected = None;
                    return false;
                }
                KeyCode::Left => {
                    if let Some(row) = rows.get(current) {
                        if !row.represents_pane()
                            && !self.sidebar_tree.collapsed.contains(&row.target)
                        {
                            self.sidebar_tree.collapsed.push(row.target.clone());
                        } else if let Some(parent) = rows[..current]
                            .iter()
                            .rev()
                            .find(|parent| parent.depth < row.depth)
                        {
                            self.sidebar_tree.selected = Some(parent.target.clone());
                        }
                    }
                }
                KeyCode::Right => {
                    if let Some(row) = rows.get(current) {
                        if let ClientNavigatorTarget::Machine { endpoint_id } = &row.target {
                            self.collapsed_endpoints.remove(endpoint_id);
                        }
                        if let Some(index) = self
                            .sidebar_tree
                            .collapsed
                            .iter()
                            .position(|target| target == &row.target)
                        {
                            self.sidebar_tree.collapsed.remove(index);
                        } else if let Some(child) = rows
                            .get(current + 1)
                            .filter(|child| child.depth > row.depth)
                        {
                            self.sidebar_tree.selected = Some(child.target.clone());
                        }
                    }
                }
                KeyCode::Enter => {
                    if let Some(row) = rows.get(current) {
                        self.focus_sidebar_target(row.target.clone(), outcome);
                    }
                }
                _ => return false,
            }
        } else if modifiers == KeyModifiers::CONTROL && matches!(code, KeyCode::Char('d' | 'u')) {
            self.move_sidebar_selection(if code == KeyCode::Char('d') { 8 } else { -8 });
        } else {
            return false;
        }
        outcome.repaint = true;
        true
    }

    fn toggle_sidebar_target(&mut self, target: ClientNavigatorTarget) {
        let removed_endpoint = match &target {
            ClientNavigatorTarget::Machine { endpoint_id } => {
                self.collapsed_endpoints.remove(endpoint_id)
            }
            _ => false,
        };
        if let Some(index) = self
            .sidebar_tree
            .collapsed
            .iter()
            .position(|candidate| candidate == &target)
        {
            self.sidebar_tree.collapsed.remove(index);
        } else if !removed_endpoint {
            self.sidebar_tree.collapsed.push(target);
        }
    }

    pub(super) fn handle_sidebar_tree_click(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some((rect, target)) = self
            .hits
            .sidebar_tree_rows
            .iter()
            .find(|(rect, _)| super::contains(*rect, point))
            .cloned()
        else {
            return false;
        };
        if self.hits.workspaces.iter().any(|hit| {
            hit.group_toggle
                .as_ref()
                .is_some_and(|(rect, _)| super::contains(*rect, point))
        }) {
            return false;
        }
        let row = self
            .sidebar_rows()
            .into_iter()
            .find(|row| row.target == target);
        let toggle = row.is_some_and(|row| {
            !row.represents_pane() && point.0 == rect.x + 3 + u16::from(row.depth) * 2
        });
        self.sidebar_tree.selected = Some(target.clone());
        if toggle {
            self.toggle_sidebar_target(target);
            outcome.repaint = true;
        } else if matches!(target, ClientNavigatorTarget::Workspace { .. }) {
            return false; // Preserve workspace press/release and drag behavior.
        } else {
            self.focus_sidebar_target(target, outcome);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> ClientShellState {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(super::super::tests::snapshot()));
        state
    }

    fn split_shell() -> ClientShellState {
        let mut state = shell();
        let mut snapshot = super::super::tests::snapshot();
        let mut pane = snapshot.panes[0].clone();
        pane.pane_id = "pane_2".into();
        snapshot.panes.push(pane);
        state.set_snapshot(Box::new(snapshot));
        state
    }

    #[test]
    fn single_tab_single_pane_merges_into_space_row() {
        let mut state = shell();
        let rows = state.sidebar_rows();
        assert_eq!(rows.len(), 1);
        assert!(matches!(
            rows[0].target,
            ClientNavigatorTarget::Workspace { .. }
        ));
        assert_eq!(rows[0].label, "client-shell");
        assert_eq!(rows[0].sole_pane_id.as_deref(), Some("pane_1"));
        assert!(rows[0].represents_pane());
        assert!(rows[0].current);
        assert_eq!(rows[0].depth, 0);
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.selected = Some(rows[0].target.clone());
        let mut outcome = ClientShellInput::default();
        for code in [KeyCode::Left, KeyCode::Right, KeyCode::Char(' ')] {
            state.route_sidebar_navigation(
                &crate::input::TerminalKey::new(code, KeyModifiers::empty()),
                &mut outcome,
            );
        }
        assert!(state.sidebar_tree.collapsed.is_empty());
        assert!(outcome.actions.is_empty());
        assert!(state.jump_sidebar(0, &mut outcome));
        assert!(
            matches!(outcome.actions.as_slice(), [ClientShellAction::Endpoint { request, .. }] if matches!(&request.method, crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1"))
        );
    }

    #[test]
    fn only_tab_uses_space_header_with_both_panes_beneath() {
        let mut state = shell();
        let mut snapshot = super::super::tests::snapshot();
        let mut pane = snapshot.panes[0].clone();
        pane.pane_id = "pane_2".into();
        snapshot.panes.push(pane);
        state.set_snapshot(Box::new(snapshot));
        let rows = state.sidebar_rows();
        assert_eq!(rows.len(), 3);
        assert!(rows
            .iter()
            .all(|row| !matches!(row.target, ClientNavigatorTarget::Tab { .. })));
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].depth, 1);
        assert_eq!(rows.iter().filter(|row| row.represents_pane()).count(), 2);
    }

    #[test]
    fn multiple_tabs_only_show_headers_for_tabs_with_multiple_panes() {
        let mut state = shell();
        let mut snapshot = super::super::tests::snapshot();
        snapshot.tabs[0].label = "hidden single-pane tab".into();
        snapshot.tabs[0].custom_label = true;
        snapshot.panes[0].label = Some("first pane".into());
        let mut split_tab = snapshot.tabs[0].clone();
        split_tab.tab_id = "tab_2".into();
        split_tab.label = "split tab".into();
        snapshot.tabs.push(split_tab);
        for name in ["second pane", "third pane"] {
            let mut pane = snapshot.panes[0].clone();
            pane.pane_id = name.into();
            pane.tab_id = "tab_2".into();
            pane.label = Some(name.into());
            snapshot.panes.push(pane);
        }
        state.set_snapshot(Box::new(snapshot));
        let rows = state.sidebar_rows();
        assert_eq!(
            rows.iter()
                .map(|row| (row.depth, row.label.as_str()))
                .collect::<Vec<_>>(),
            [
                (0, "client-shell"),
                (1, "first pane"),
                (1, "split tab"),
                (2, "second pane"),
                (2, "third pane")
            ]
        );
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.selected = Some(rows[3].target.clone());
        let left = crate::input::TerminalKey::new(KeyCode::Left, KeyModifiers::empty());
        let mut outcome = ClientShellInput::default();
        state.route_sidebar_navigation(&left, &mut outcome);
        assert_eq!(state.sidebar_tree.selected, Some(rows[2].target.clone()));
        state.route_sidebar_navigation(&left, &mut outcome);
        assert_eq!(state.sidebar_rows().len(), 3);
        assert!(outcome.actions.is_empty());
    }

    #[test]
    fn name_only_tree_keeps_parent_labels_separate_and_hides_agent_types() {
        let config: Config = toml::from_str(
            r#"
[ui]
hide_tab_bar = true
[ui.sidebar.spaces]
rows = [["state_icon", "workspace"]]
[ui.sidebar.agents]
rows = [["state_icon", "pane"]]
"#,
        )
        .unwrap();
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
        let mut snapshot = super::super::tests::snapshot();
        snapshot.workspaces[0].label = "project".into();
        snapshot.tabs[0].label = "development".into();
        snapshot.tabs[0].custom_label = true;
        snapshot.panes[0].label = Some("fix-login".into());
        let mut second_pane = snapshot.panes[0].clone();
        second_pane.pane_id = "pane_2".into();
        second_pane.label = Some("other pane".into());
        snapshot.panes.push(second_pane);
        snapshot.agents.push(crate::protocol::ClientShellAgent {
            pane_id: "pane_1".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some("codex".into()),
            display_agent: Some("Codex".into()),
            agent: Some("codex".into()),
            title: Some("old agent title".into()),
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: crate::api::schema::AgentStatus::Working,
            state_change_seq: 1,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: true,
        });
        state.set_snapshot(Box::new(snapshot.clone()));
        assert_eq!(
            state
                .sidebar_rows()
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>(),
            ["project", "fix-login", "other pane"]
        );
        let frame = state.compose(120, 30).unwrap();
        for (rect, target) in &state.hits.sidebar_tree_rows {
            let label = match target {
                ClientNavigatorTarget::Workspace { .. } => "project",
                ClientNavigatorTarget::Tab { .. } => "development",
                ClientNavigatorTarget::Pane { pane_id, .. } => {
                    if pane_id == "pane_1" {
                        "fix-login"
                    } else {
                        "other pane"
                    }
                }
                ClientNavigatorTarget::Machine { .. } => continue,
            };
            let line = (rect.x..rect.right())
                .map(|x| {
                    frame.cells[usize::from(rect.y) * usize::from(frame.width) + usize::from(x)]
                        .symbol
                        .as_str()
                })
                .collect::<String>();
            assert!(line.trim_end().ends_with(label), "{line}");
            assert!(
                !line.contains(" · ")
                    && !line.contains("Codex")
                    && !line.contains("old agent title"),
                "{line}"
            );
        }
        snapshot.panes[0].label = None;
        snapshot.agents[0].title = Some("pane title".into());
        state.set_snapshot(Box::new(snapshot.clone()));
        assert_eq!(state.sidebar_rows()[1].label, "pane title");
        snapshot.agents[0].title = None;
        state.set_snapshot(Box::new(snapshot));
        assert_eq!(state.sidebar_rows()[1].label, "pane 1");
    }

    #[test]
    fn collapsing_workspace_hides_descendants_search_reveals_them() {
        let mut state = split_shell();
        state
            .sidebar_tree
            .collapsed
            .push(state.sidebar_rows()[0].target.clone());
        assert_eq!(state.sidebar_rows().len(), 1);
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.query = "repo".into();
        assert_eq!(state.sidebar_rows().len(), 3);
    }

    #[test]
    fn left_from_pane_selects_parent_then_collapses_it() {
        let mut state = split_shell();
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.selected = Some(state.sidebar_rows()[1].target.clone());
        let key = crate::input::TerminalKey::new(KeyCode::Char('h'), KeyModifiers::empty());
        let mut outcome = ClientShellInput::default();
        assert!(state.route_sidebar_navigation(&key, &mut outcome));
        assert_eq!(
            state.sidebar_tree.selected,
            Some(state.sidebar_rows()[0].target.clone())
        );
        state.route_sidebar_navigation(&key, &mut outcome);
        assert_eq!(state.sidebar_rows().len(), 1);
    }

    #[test]
    fn numeric_prompt_consumes_typing_and_rejects_zero() {
        let mut state = shell();
        let mut outcome = ClientShellInput::default();
        state.sidebar_tree.jump = Some(String::new());
        assert!(state.route_sidebar_prompt(
            &crate::input::TerminalKey::new(KeyCode::Char('0'), KeyModifiers::empty()),
            &mut outcome
        ));
        assert!(state.route_sidebar_prompt(
            &crate::input::TerminalKey::new(KeyCode::Enter, KeyModifiers::empty()),
            &mut outcome
        ));
        assert!(outcome.actions.is_empty());
        assert_eq!(state.sidebar_tree.jump.as_deref(), Some("0"));
    }
    #[test]
    fn configured_pane_keys_move_cursor_without_focusing_and_leaf_right_is_noop() {
        let mut state = split_shell();
        state.mode = ClientShellMode::Navigate;
        let mut outcome = ClientShellInput::default();
        let workspace = state.sidebar_rows()[0].target.clone();
        state.sidebar_tree.selected = Some(workspace);
        state.route_sidebar_navigation(
            &crate::input::TerminalKey::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &mut outcome,
        );
        assert!(matches!(
            state.sidebar_tree.selected,
            Some(ClientNavigatorTarget::Pane { .. })
        ));
        state.route_sidebar_navigation(
            &crate::input::TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()),
            &mut outcome,
        );
        assert_eq!(state.mode, ClientShellMode::Navigate);
        assert!(outcome.actions.is_empty());
    }

    #[test]
    fn search_paste_and_escape_preserve_filter_then_jump_prompt() {
        let mut state = shell();
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.searching = true;
        let mut outcome = ClientShellInput::default();
        assert!(state.insert_sidebar_text("repo"));
        assert!(state.modal_paste_target_active());
        state.route_sidebar_prompt(
            &crate::input::TerminalKey::new(KeyCode::Esc, KeyModifiers::empty()),
            &mut outcome,
        );
        assert!(!state.sidebar_tree.searching);
        assert_eq!(state.sidebar_tree.query, "repo");
        state.handle_sidebar_action(
            crate::input::KeybindAction::JumpSidebarItemPrompt,
            &mut outcome,
        );
        assert_eq!(state.sidebar_tree.query, "repo");
        assert_eq!(state.mode, ClientShellMode::Navigate);
        assert_eq!(state.sidebar_tree.jump.as_deref(), Some(""));
    }

    #[test]
    fn empty_search_cannot_close_previously_selected_workspace() {
        let mut state = shell();
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.selected = Some(state.sidebar_rows()[0].target.clone());
        state.sidebar_tree.query = "no-such-workspace".into();
        assert!(!state.prepare_sidebar_workspace_action(&mut ClientShellInput::default()));
    }

    #[test]
    fn multiword_filter_matches_separate_fields_and_is_not_applied_in_terminal_mode() {
        let mut state = shell();
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.query = "pane repo".into();
        assert_eq!(state.sidebar_rows().len(), 1);
        state.sidebar_tree.query = "missing".into();
        assert!(state.sidebar_rows().is_empty());
        state.mode = ClientShellMode::Terminal;
        assert_eq!(state.sidebar_rows().len(), 1);
    }

    #[test]
    fn duplicate_remote_pane_ids_remain_qualified_when_jumping() {
        let mut state = shell();
        let profile = SavedSshEndpoint {
            id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
                .unwrap(),
            label: "Remote".into(),
            target: "dev@example.invalid".into(),
            session: "agents".into(),
            enabled: true,
        };
        let remote = ClientEndpointId::Ssh(profile.id.clone());
        state.set_endpoint_catalog(&[profile]);
        state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
        state.set_endpoint_snapshot(&remote, Box::new(super::super::tests::snapshot()));
        let mut outcome = ClientShellInput::default();
        assert!(state.jump_sidebar(1, &mut outcome));
        assert!(
            matches!(outcome.actions.as_slice(), [ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Pane(pane_id)) }] if endpoint_id == &remote && pane_id == "pane_1")
        );
    }

    #[test]
    fn navigating_remote_workspace_cannot_mutate_local_workspace_with_same_id() {
        let mut state = shell();
        let profile = SavedSshEndpoint {
            id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
                .unwrap(),
            label: "Remote".into(),
            target: "dev@example.invalid".into(),
            session: "agents".into(),
            enabled: true,
        };
        let remote = ClientEndpointId::Ssh(profile.id.clone());
        state.set_endpoint_catalog(&[profile]);
        state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
        state.set_endpoint_snapshot(&remote, Box::new(super::super::tests::snapshot()));
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.selected = Some(ClientNavigatorTarget::Workspace {
            endpoint_id: remote,
            workspace_id: "ws_1".into(),
        });
        let mut outcome = ClientShellInput::default();
        assert!(!state.prepare_sidebar_workspace_action(&mut outcome));
        assert!(outcome.actions.is_empty());
    }
    #[test]
    fn empty_rendered_filter_disables_workspace_mutations_without_falling_back() {
        let mut state = shell();
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.selected = Some(state.sidebar_rows()[0].target.clone());
        state.sidebar_tree.query = "nothing-matches".into();
        state.compose(100, 28).unwrap();
        assert!(state.sidebar_tree.selected.is_none());
        let mut outcome = ClientShellInput::default();
        assert!(!state.prepare_sidebar_workspace_action(&mut outcome));
        assert!(outcome.actions.is_empty());
    }

    #[test]
    fn prefixed_number_prompt_preserves_navigate_mode_and_filter() {
        let mut state = shell();
        state.config.keybinds.keybinds.jump_sidebar_item_prompt =
            crate::config::ActionKeybinds::prefix("f9");
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.query = "repo".into();
        let outcome = state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Key(
            crate::input::TerminalKey::new(KeyCode::F(9), KeyModifiers::empty()),
        )]);
        assert_eq!(state.mode, ClientShellMode::Navigate);
        assert_eq!(state.sidebar_tree.query, "repo");
        assert_eq!(state.sidebar_tree.jump.as_deref(), Some(""));
        assert!(outcome.actions.is_empty());
    }

    fn remote_shell() -> (ClientShellState, ClientEndpointId) {
        let mut state = shell();
        let profile = SavedSshEndpoint {
            id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
                .unwrap(),
            label: "Remote".into(),
            target: "dev@example.invalid".into(),
            session: "agents".into(),
            enabled: true,
        };
        let remote = ClientEndpointId::Ssh(profile.id.clone());
        state.set_endpoint_catalog(&[profile]);
        state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
        state.set_endpoint_snapshot(&remote, Box::new(super::super::tests::snapshot()));
        (state, remote)
    }

    #[test]
    fn unavailable_numbered_pane_leaves_prompt_open_and_does_not_focus_local_id() {
        let (mut state, remote) = remote_shell();
        state.set_endpoint_status(&remote, ClientEndpointStatus::Reconnecting);
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.jump = Some("2".into());
        let mut outcome = ClientShellInput::default();
        state.route_sidebar_prompt(
            &crate::input::TerminalKey::new(KeyCode::Enter, KeyModifiers::empty()),
            &mut outcome,
        );
        assert_eq!(state.sidebar_tree.jump.as_deref(), Some("2"));
        assert_eq!(state.mode, ClientShellMode::Navigate);
        assert!(outcome.actions.is_empty());
        assert!(outcome.repaint);
    }

    #[test]
    fn upstream_machine_collapse_is_respected_and_expands_with_tree_keyboard() {
        let (mut state, remote) = remote_shell();
        state.collapsed_endpoints.insert(remote.clone());
        let before = state.sidebar_rows().len();
        state.mode = ClientShellMode::Navigate;
        state.sidebar_tree.selected = Some(ClientNavigatorTarget::Machine {
            endpoint_id: remote.clone(),
        });
        state.route_sidebar_navigation(
            &crate::input::TerminalKey::new(KeyCode::Right, KeyModifiers::empty()),
            &mut ClientShellInput::default(),
        );
        assert!(!state.collapsed_endpoints.contains(&remote));
        assert_eq!(state.sidebar_rows().len(), before + 1);
    }

    #[test]
    fn upstream_machine_collapse_expands_with_mouse_marker() {
        let (mut state, remote) = remote_shell();
        state.collapsed_endpoints.insert(remote.clone());
        state.compose(100, 28).unwrap();
        let rect = state
            .hits
            .sidebar_tree_rows
            .iter()
            .find(|(_, target)| {
                *target
                    == ClientNavigatorTarget::Machine {
                        endpoint_id: remote.clone(),
                    }
            })
            .unwrap()
            .0;
        assert!(
            state.handle_sidebar_tree_click((rect.x + 3, rect.y), &mut ClientShellInput::default())
        );
        assert!(!state.collapsed_endpoints.contains(&remote));
        assert!(!state
            .sidebar_tree
            .collapsed
            .contains(&ClientNavigatorTarget::Machine {
                endpoint_id: remote
            }));
    }
}
