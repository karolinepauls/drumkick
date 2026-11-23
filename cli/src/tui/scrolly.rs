use std::num::TryFromIntError;

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    widgets::{Scrollbar, ScrollbarState},
};

pub enum ScrollyNav {
    Step(isize),
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavOutcome {
    pub index: usize,
    pub condition: NavCondition,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NavCondition {
    Underflow(usize),
    Normal,
    Overflow(usize),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ScrollyParams {
    pub visible_height: u16,
    pub item_height: u16,
    pub item_count: usize,
}

impl ScrollyParams {
    fn content_height(&self) -> u16 {
        (self.item_count as u16).saturating_mul(self.item_height)
    }
}

#[derive(Debug)]
pub struct Scrolly {
    scroll_offset: u16, // state
}

impl Scrolly {
    pub fn new() -> Self {
        Self { scroll_offset: 0 }
    }

    /// Needs to be called
    /// - whenever visible height changes
    /// - whenever active index changes (unless navigating via `navigate`).
    pub fn adjust_scroll_offset_for_selection(&mut self, params: ScrollyParams, index: usize) {
        let selected_item_top = (index as u16).saturating_mul(params.item_height);
        let selected_item_bottom = selected_item_top.saturating_add(params.item_height);

        let display_top = self.scroll_offset;
        let display_bottom = self.scroll_offset.saturating_add(params.visible_height);

        if selected_item_bottom > display_bottom {
            self.scroll_offset = selected_item_bottom.saturating_sub(params.visible_height);
        } else if selected_item_top < display_top {
            self.scroll_offset = selected_item_top;
        }
    }

    pub fn navigate(
        &mut self,
        params: ScrollyParams,
        start_index: isize,
        dir: ScrollyNav,
    ) -> NavOutcome {
        let items_per_page = params.visible_height / params.item_height;
        let outcome = match dir {
            ScrollyNav::Step(step) => self.navigate_by_offset(params, start_index, step),
            ScrollyNav::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(params.visible_height);
                self.navigate_by_offset(params, start_index, -(items_per_page as isize))
            }
            ScrollyNav::PageDown => {
                self.scroll_offset = self
                    .scroll_offset
                    .saturating_add(params.visible_height)
                    .min(
                        params
                            .content_height()
                            .saturating_sub(params.visible_height),
                    );

                self.navigate_by_offset(params, start_index, items_per_page as isize)
            }
        };
        self.adjust_scroll_offset_for_selection(params, outcome.index);
        outcome
    }

    fn navigate_by_offset(
        &self,
        params: ScrollyParams,
        start_index: isize,
        step: isize,
    ) -> NavOutcome {
        let last_index = params.item_count as isize - 1; // Can actually be -1 for an empty list.
        let new_index_unbounded = start_index + step;
        let new_index_under_max_bound = new_index_unbounded.clamp(isize::MIN, last_index);

        let condition = if new_index_under_max_bound < 0 {
            // Underflow can happen if navigating before the initial element or if there are no
            // elements.
            NavCondition::Underflow(-new_index_under_max_bound as usize)
        } else if new_index_unbounded > last_index {
            NavCondition::Overflow((new_index_unbounded - last_index) as usize)
        } else {
            NavCondition::Normal
        };

        NavOutcome {
            index: usize::try_from(new_index_under_max_bound).unwrap_or(0),
            condition,
        }
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        inner_rect: Rect,
        params: &ScrollyParams,
        render_content: impl FnOnce(&mut Buffer, &[Rect]),
    ) {
        let measures_and_scrollbar_layout =
            Layout::horizontal(vec![Constraint::Fill(1), Constraint::Length(1)]).split(inner_rect);
        let measures_rect = measures_and_scrollbar_layout[0];
        let scrollbar_rect = measures_and_scrollbar_layout[1];

        // Scrollbar
        let mut scrollbar_state = ScrollbarState::new(
            // Weird but needed to make the scrollbar act correctly. The content length passed into
            // the scrollbar has to be "the length that doesn't fit".
            params
                .content_height()
                .saturating_sub(measures_rect.height)
                .into(),
        )
        .position(self.scroll_offset.into())
        .viewport_content_length(measures_rect.height.into());
        frame.render_stateful_widget(Scrollbar::default(), scrollbar_rect, &mut scrollbar_state);

        // Scrolled measure list
        let scroll_buf_rect = Rect::new(0, 0, measures_rect.width, params.content_height());
        let mut content_buf = Buffer::empty(scroll_buf_rect);

        // Calculate the layout by hand. This is much faster than using Layout. With Layout it takes
        // over 260ms for 225 elements whenever the list changes (as there seems to be a layout
        // cache).
        let item_rects: Vec<_> = (0..(params.item_count))
            .map(|i| {
                Rect::new(
                    0,
                    (i as u16) * params.item_height,
                    scroll_buf_rect.width,
                    params.item_height,
                )
            })
            .collect();

        // Content
        render_content(&mut content_buf, &item_rects);

        // Write the visible section of the scroll buffer into the visible content.
        let visible_content = content_buf
            .content
            .into_iter()
            .skip((measures_rect.width.saturating_mul(self.scroll_offset)) as usize)
            .take(measures_rect.area() as usize);
        for (i, cell) in visible_content.enumerate() {
            let x = i as u16 % measures_rect.width;
            let y = i as u16 / measures_rect.width;
            frame.buffer_mut()[(measures_rect.x + x, measures_rect.y + y)] = cell;
        }
    }
}
