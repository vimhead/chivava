use crossterm::event::KeyCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    Single,
    Multiple,
}

pub struct Choice {
    pub label: String,
    pub is_selected: bool,
}

pub struct SelectionPrompt {
    pub title: String,
    pub mode: SelectionMode,
    pub choices: Vec<Choice>,
    pub focused: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SelectionOutcome {
    Pending,
    Confirmed,
    Cancelled,
}

impl SelectionPrompt {
    pub fn handle_key(&mut self, key: KeyCode) -> SelectionOutcome {
        if key == KeyCode::Esc {
            return SelectionOutcome::Cancelled;
        }
        if self.choices.is_empty() {
            return SelectionOutcome::Pending;
        }
        let count = self.choices.len();
        match key {
            KeyCode::Down | KeyCode::Char('j') => self.focused = (self.focused + 1) % count,
            KeyCode::Up | KeyCode::Char('k') => self.focused = (self.focused + count - 1) % count,
            KeyCode::PageDown => self.focused = (self.focused + 6).min(count - 1),
            KeyCode::PageUp => self.focused = self.focused.saturating_sub(6),
            KeyCode::Home => self.focused = 0,
            KeyCode::End => self.focused = count - 1,
            KeyCode::Char(' ') if self.mode == SelectionMode::Multiple => {
                self.choices[self.focused].is_selected = !self.choices[self.focused].is_selected;
            }
            KeyCode::Char('a') if self.mode == SelectionMode::Multiple => {
                let should_select_all = !self.choices.iter().all(|choice| choice.is_selected);
                for choice in &mut self.choices {
                    choice.is_selected = should_select_all;
                }
            }
            KeyCode::Enter => return SelectionOutcome::Confirmed,
            _ => {}
        }
        SelectionOutcome::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_prompt(mode: SelectionMode) -> SelectionPrompt {
        SelectionPrompt {
            title: "Theme".into(),
            mode,
            focused: 0,
            choices: (0..48)
                .map(|index| Choice {
                    label: format!("Theme {index}"),
                    is_selected: index == 0,
                })
                .collect(),
        }
    }

    #[test]
    fn single_selection_supports_wrap_paging_and_explicit_confirmation() {
        let mut prompt = create_prompt(SelectionMode::Single);
        assert_eq!(prompt.handle_key(KeyCode::Up), SelectionOutcome::Pending);
        assert_eq!(prompt.focused, 47);
        prompt.handle_key(KeyCode::Down);
        assert_eq!(prompt.focused, 0);
        prompt.handle_key(KeyCode::PageDown);
        assert_eq!(prompt.focused, 6);
        prompt.handle_key(KeyCode::PageUp);
        assert_eq!(prompt.focused, 0);
        prompt.handle_key(KeyCode::End);
        assert_eq!(prompt.focused, 47);
        assert_eq!(
            prompt.handle_key(KeyCode::Enter),
            SelectionOutcome::Confirmed
        );
        assert_eq!(prompt.handle_key(KeyCode::Esc), SelectionOutcome::Cancelled);
    }

    #[test]
    fn multiselect_toggles_only_the_focused_choice() {
        let mut prompt = create_prompt(SelectionMode::Multiple);
        prompt.handle_key(KeyCode::Down);
        prompt.handle_key(KeyCode::Char(' '));
        assert!(prompt.choices[0].is_selected);
        assert!(prompt.choices[1].is_selected);
        assert!(!prompt.choices[2].is_selected);
        prompt.handle_key(KeyCode::Char('a'));
        assert!(prompt.choices.iter().all(|choice| choice.is_selected));
        prompt.handle_key(KeyCode::Char('a'));
        assert!(prompt.choices.iter().all(|choice| !choice.is_selected));
    }
}
