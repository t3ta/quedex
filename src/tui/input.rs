use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy)]
pub enum Action {
    Quit,
    Up,
    Down,
    ToggleLogFocus,
    ToggleStream,
    Retry,
    CancelTask,
    CancelRun,
    ToggleGraph,
}

pub fn handle_key(event: KeyEvent) -> Option<Action> {
    match event.code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Up => Some(Action::Up),
        KeyCode::Down => Some(Action::Down),
        KeyCode::Enter => Some(Action::ToggleLogFocus),
        KeyCode::Char('t') => Some(Action::ToggleStream),
        KeyCode::Char('r') => Some(Action::Retry),
        KeyCode::Char('c') if !event.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Action::CancelTask)
        }
        KeyCode::Char('C') => Some(Action::CancelRun),
        KeyCode::Char('g') => Some(Action::ToggleGraph),
        _ => None,
    }
}
