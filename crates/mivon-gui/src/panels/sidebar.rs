//! Sidebar kiri: tab Project (file tree navigabel), Symbols, Architecture,
//! Dependency, Search.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use super::super::state::{classify_file, FileCat, GuiState, SidebarTab};

pub fn show(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let block = Block::default()
        .title("Sidebar")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::DarkGray));
    f.render_widget(block, area);

    let inner = area.inner(&ratatui::layout::Margin {
        vertical: 1u16,
        horizontal: 1u16,
    });
    if inner.height < 3 {
        return;
    }

    // Tab selector.
    let tabs = ["Project", "Symbols", "Architecture", "Dependency", "Search"];
    let tab_items: Vec<ListItem> = tabs
        .iter()
        .map(|t| {
            let active = match *t {
                "Project" => state.sidebar_tab == SidebarTab::Project,
                "Symbols" => state.sidebar_tab == SidebarTab::Symbols,
                "Architecture" => state.sidebar_tab == SidebarTab::Architecture,
                "Dependency" => state.sidebar_tab == SidebarTab::Dependency,
                "Search" => state.sidebar_tab == SidebarTab::Search,
                _ => false,
            };
            ListItem::new(format!("  {}", t)).style(if active {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            })
        })
        .collect();
    let tabs_list =
        List::new(tab_items).block(Block::default().borders(Borders::BOTTOM).title("Tabs"));
    f.render_widget(tabs_list, inner);

    // Konten tab (di bawah tab selector).
    let content_area = ratatui::layout::Rect {
        x: inner.x,
        y: inner.y.saturating_add(tabs.len() as u16 + 1),
        width: inner.width,
        height: inner.height.saturating_sub(tabs.len() as u16 + 1),
    };

    match state.sidebar_tab {
        SidebarTab::Project => project_tab(f, content_area, state),
        SidebarTab::Symbols => symbols_tab(f, content_area, state),
        SidebarTab::Architecture => architecture_tab(f, content_area, state),
        SidebarTab::Dependency => dependency_tab(f, content_area, state),
        SidebarTab::Search => search_tab(f, content_area, state),
    }
}

/// Navigasi sidebar: pindah seleksi atas/bawah.
pub fn sidebar_select_next(state: &mut GuiState) {
    state.sidebar_selected = state.sidebar_selected.saturating_add(1);
}

pub fn sidebar_select_prev(state: &mut GuiState) {
    state.sidebar_selected = state.sidebar_selected.saturating_sub(1);
}

/// Buka file yang sedang terpilih di sidebar project tab.
pub fn sidebar_open_selected(state: &mut GuiState) {
    if let Some(path) = collect_sv_paths(state).get(state.sidebar_selected) {
        state.open_file(path.clone());
    }
}

/// Kumpulkan semua path file .sv/.svh dari pohon project (flat, urut tree).
fn collect_sv_paths(state: &GuiState) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    collect_sv_paths_rec(&state.files, &mut out);
    out
}

fn collect_sv_paths_rec(nodes: &[crate::state::FileNode], out: &mut Vec<std::path::PathBuf>) {
    for node in nodes {
        if node.is_dir {
            collect_sv_paths_rec(&node.children, out);
        } else {
            let ext = node.path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "sv" || ext == "svh" {
                out.push(node.path.clone());
            }
        }
    }
}

fn project_tab(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    if state.project_name.is_empty() {
        let p = Paragraph::new("Buka project (Ctrl+O) untuk melihat file").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, area);
        return;
    }

    let paths = collect_sv_paths(state);
    if paths.is_empty() {
        let p = Paragraph::new("Tidak ada file .sv/.svh").style(Style::default().fg(Color::Gray));
        f.render_widget(p, area);
        return;
    }

    // Clamp seleksi ke jumlah item.
    if state.sidebar_selected >= paths.len() {
        state.sidebar_selected = paths.len().saturating_sub(1);
    }

    let items: Vec<ListItem> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let cat = classify_file(p);
            let color = match cat {
                FileCat::Rtl => Color::Blue,
                FileCat::Testbench => Color::Green,
                FileCat::Library => Color::Magenta,
                FileCat::Include => Color::Cyan,
                FileCat::All => Color::Gray,
            };
            let is_bm = state.bookmarks.contains(p);
            let star = if is_bm { "★" } else { "☆" };
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| p.display().to_string());
            let selected = i == state.sidebar_selected;
            ListItem::new(format!(" {} {}", star, name)).style(
                Style::default().fg(color).add_modifier(if selected {
                    Modifier::REVERSED
                } else {
                    Modifier::empty()
                }),
            )
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(state.sidebar_selected));
    let list = List::new(items);
    f.render_stateful_widget(list, area, &mut list_state);
}

fn symbols_tab(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let Some(info) = &state.compile_info else {
        let p = Paragraph::new("Compile dulu untuk melihat symbols").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, area);
        return;
    };

    let mut items: Vec<ListItem> = Vec::new();
    if !info.modules.is_empty() {
        items.push(
            ListItem::new(" Modules").style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        );
        for m in &info.modules {
            items
                .push(ListItem::new(format!("  ▸ {}", m)).style(Style::default().fg(Color::White)));
        }
        items.push(ListItem::new(""));
    }
    if !info.packages.is_empty() {
        items.push(
            ListItem::new(" Packages").style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        );
        for m in &info.packages {
            items
                .push(ListItem::new(format!("  ◈ {}", m)).style(Style::default().fg(Color::White)));
        }
        items.push(ListItem::new(""));
    }
    if !info.interfaces.is_empty() {
        items.push(
            ListItem::new(" Interfaces").style(
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
        );
        for m in &info.interfaces {
            items
                .push(ListItem::new(format!("  ◇ {}", m)).style(Style::default().fg(Color::White)));
        }
    }

    if items.is_empty() {
        items.push(ListItem::new("(kosong)").style(Style::default().fg(Color::Gray)));
    }

    let list = List::new(items);
    f.render_widget(list, area);
}

fn architecture_tab(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let Some(info) = &state.compile_info else {
        let p = Paragraph::new("Compile dulu untuk melihat architecture").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, area);
        return;
    };

    let mut items: Vec<ListItem> = Vec::new();
    items.push(
        ListItem::new("Modules & instances").style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    );
    for m in &info.modules {
        items.push(ListItem::new(format!("  {}", m)).style(Style::default().fg(Color::White)));
    }
    if items.len() <= 1 {
        items.push(ListItem::new("(belum ada data)").style(Style::default().fg(Color::Gray)));
    }

    let list = List::new(items);
    f.render_widget(list, area);
}

fn dependency_tab(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let Some(info) = &state.compile_info else {
        let p = Paragraph::new("Compile dulu untuk melihat dependency").style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );
        f.render_widget(p, area);
        return;
    };

    let mut items: Vec<ListItem> = Vec::new();
    items.push(
        ListItem::new("Dependency graph").style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    );
    for dep in &info.deps {
        items.push(
            ListItem::new(format!("  {}", dep.module)).style(Style::default().fg(Color::White)),
        );
        for (child, count) in &dep.children {
            items.push(
                ListItem::new(format!("    → {} (x{})", child, count))
                    .style(Style::default().fg(Color::Cyan)),
            );
        }
    }
    if items.len() <= 1 {
        items.push(ListItem::new("(belum ada data)").style(Style::default().fg(Color::Gray)));
    }

    let list = List::new(items);
    f.render_widget(list, area);
}

fn search_tab(f: &mut Frame, area: ratatui::layout::Rect, state: &mut GuiState) {
    let mut items: Vec<ListItem> = Vec::new();
    items.push(
        ListItem::new("Search categories").style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    );
    items.push(
        ListItem::new("  Module / Signal / Parameter / Package / Macro / Instance / Text")
            .style(Style::default().fg(Color::White)),
    );
    items.push(ListItem::new(""));
    items.push(
        ListItem::new(format!("Filter: {}", state.search_filter))
            .style(Style::default().fg(Color::Cyan)),
    );
    items.push(
        ListItem::new(format!("Text hits: {}", state.text_hits.len()))
            .style(Style::default().fg(Color::White)),
    );
    if items.len() <= 2 {
        items.push(ListItem::new("(kosong)").style(Style::default().fg(Color::Gray)));
    }

    let list = List::new(items);
    f.render_widget(list, area);
}
