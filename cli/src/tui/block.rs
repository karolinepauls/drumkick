use ratatui::widgets::{Block, Borders};

pub const DEFAULT_BLOCK_PADDING: u16 = 1;

pub fn subview_block(title: &str) -> Block<'_> {
    Block::new()
        .padding(DEFAULT_BLOCK_PADDING.into())
        .borders(Borders::TOP)
        .title(format!("───{title}"))
}
