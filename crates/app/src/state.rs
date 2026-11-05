//! App-wide actions (keyboard shortcuts bind to these, see `sidebar_model::KEYMAP`).

use gpui::actions;

actions!(
    rb,
    [
        Quit,
        ToggleSidebar,
        NewTab,
        CloseTab,
        FocusAddress,
        GoBack,
        GoForward,
        Select1,
        Select2,
        Select3,
        Select4,
        Select5,
        Select6,
        Select7,
        Select8,
        Select9,
    ]
);
