use crate::app::state::NavigatorRow;

pub(super) fn tree_prefix(rows: &[NavigatorRow], idx: usize) -> String {
    tree_prefix_inner(rows, idx, true)
}

#[cfg(test)]
pub(super) fn fixed_open_tree_prefix(rows: &[NavigatorRow], idx: usize) -> String {
    tree_prefix_inner(rows, idx, false)
}

/// Tree prefix for the interactive sidebar, where both workspace and tab rows
/// are real expandable nodes.
pub(super) fn sidebar_tree_prefix(rows: &[NavigatorRow], idx: usize) -> String {
    let row = &rows[idx];
    let disclosure = if row.is_workspace || row.is_tab {
        if row.expanded {
            "▾"
        } else {
            "▸"
        }
    } else {
        ""
    };
    if row.depth == 0 {
        return disclosure.to_string();
    }

    let mut prefix = String::new();
    for level in 1..row.depth {
        prefix.push_str(if has_following_sibling_at_depth(rows, idx, level) {
            "│  "
        } else {
            "   "
        });
    }
    prefix.push_str(if has_following_sibling_at_depth(rows, idx, row.depth) {
        "├──"
    } else {
        "└──"
    });
    prefix.push_str(disclosure);
    prefix
}

fn tree_prefix_inner(rows: &[NavigatorRow], idx: usize, show_disclosure: bool) -> String {
    let row = &rows[idx];
    if row.is_workspace && row.depth == 0 {
        return if show_disclosure {
            if row.expanded { "▾" } else { "▸" }.to_string()
        } else {
            String::new()
        };
    }
    if row.depth == 0 {
        return "  ".to_string();
    }

    let mut prefix = String::new();
    for level in 1..row.depth {
        prefix.push_str(if has_following_sibling_at_depth(rows, idx, level) {
            "│  "
        } else {
            "   "
        });
    }
    prefix.push_str(if has_following_sibling_at_depth(rows, idx, row.depth) {
        "├──"
    } else {
        "└──"
    });
    if show_disclosure && row.is_workspace {
        prefix.push(if row.expanded { '▾' } else { '▸' });
    }
    prefix
}

pub(super) fn has_following_sibling_at_depth(rows: &[NavigatorRow], idx: usize, depth: u8) -> bool {
    rows[idx + 1..]
        .iter()
        .take_while(|row| row.depth >= depth)
        .any(|row| row.depth == depth)
}
