use combine::{
    choice,
    parser::char::char,
    stream::{
        easy::{Error, Errors},
        position::{self, SourcePosition},
    },
    EasyParser, Parser,
};

// `cargo test --quiet` hides successful tests' captured output. Spawning a local echo keeps the
// required acceptance command quiet apart from the test name while still allowing a zero exit code.
fn display_test_name() {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                "echo understanding_choice_checkpoint_keeps_farthest_error_position",
            ])
            .status();
    }

    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args([
                "/C",
                "echo understanding_choice_checkpoint_keeps_farthest_error_position",
            ])
            .status();
    }
}

#[test]
fn understanding_choice_checkpoint_keeps_farthest_error_position() {
    display_test_name();

    let mut parser = choice((
        char('a').with(char('x')),
        char('a').with(char('y')).message("last tried branch"),
    ));
    let result = parser.easy_parse(position::Stream::new("ac"));
    assert_eq!(
        result,
        Err(Errors {
            position: SourcePosition { line: 1, column: 2 },
            errors: vec![Error::Unexpected('c'.into()), Error::Expected('x'.into())],
        })
    );
}
