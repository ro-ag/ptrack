use crate::{InputEditor, Key};

#[test]
fn editor_never_splits_unicode_and_tracks_cell_width() {
    let mut editor = InputEditor::new("a界🙂");
    editor.apply(&Key::Left);
    editor.apply(&Key::Backspace);
    assert_eq!(editor.value(), "a🙂");
    assert_eq!(editor.cursor(), 1);
    editor.apply(&Key::Char('é'));
    assert_eq!(editor.value(), "aé🙂");
    let (visible, cursor) = editor.visible(3);
    assert_eq!(visible, "aé");
    assert_eq!(cursor, 2);
}

#[test]
fn editor_matches_bubble_navigation_deletion_and_paste_bindings() {
    let mut editor = InputEditor::new("alpha βeta 界🙂");
    editor.apply(&Key::CtrlLeft);
    editor.apply(&Key::AltBackspace);
    assert_eq!(editor.value(), "alpha 界🙂");
    editor.apply(&Key::AltDelete);
    assert_eq!(editor.value(), "alpha ");

    editor.apply(&Key::Paste("line\n二\t🙂".to_owned()));
    assert_eq!(editor.value(), "alpha line 二 🙂");
    editor.apply(&Key::Ctrl('a'));
    editor.apply(&Key::Ctrl('k'));
    assert_eq!(editor.value(), "");

    editor.apply(&Key::Paste("one two".to_owned()));
    editor.apply(&Key::Ctrl('w'));
    assert_eq!(editor.value(), "one ");
    editor.apply(&Key::Ctrl('u'));
    assert_eq!(editor.value(), "");
}

#[test]
fn editor_supports_emacs_character_motion_and_forward_delete() {
    let mut editor = InputEditor::new("a界🙂");
    editor.apply(&Key::Ctrl('a'));
    editor.apply(&Key::Ctrl('f'));
    editor.apply(&Key::Ctrl('d'));
    assert_eq!(editor.value(), "a🙂");
    editor.apply(&Key::Ctrl('e'));
    editor.apply(&Key::Ctrl('b'));
    editor.apply(&Key::Ctrl('h'));
    assert_eq!(editor.value(), "🙂");
}

#[test]
fn editor_matches_bubbles_control_character_sanitizer() {
    let mut editor = InputEditor::new("a\nb\rc\td\u{1b}\u{7f}\u{85}\u{fffd}界");
    assert_eq!(editor.value(), "a b c d界");

    editor.apply(&Key::Paste(
        "\u{1b}]52;c;payload\u{7}\nnext\t\u{fffd}".to_owned(),
    ));
    assert_eq!(editor.value(), "a b c d界]52;c;payload next ");

    editor.apply(&Key::Char('\u{1b}'));
    assert_eq!(editor.value(), "a b c d界]52;c;payload next ");
}
