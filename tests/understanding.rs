//! Tests backing the claims in `ANALYSIS.md` about how `choice` interacts with
//! stream checkpoints, `Commit`/`Peek` ("Consumed"/"Empty") results and error
//! merging. Every test only uses the public API; none of them reimplements the
//! internal `choice` machinery (`do_choice!` / `slice_parse_mode`).


// `cargo test --quiet` hides individual test names, so each case announces
// itself on (uncaptured) stderr to keep the verification stages visible.
fn announce(name: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "understanding case: {}", name);
}

use combine::{
    attempt, choice,
    easy::{Error, Errors, Info},
    parser::{
        char::{char, digit, string},
        repeat::many,
    },
    stream::position::{self, SourcePosition},
    EasyParser, Parser,
};

// Two branches sharing the prefix "le": "let" vs "lex".

// 1) No `attempt`: the first branch consumes 'l' and 'e' before failing, so it
//    returns `CommitErr` and `choice` never tries the second branch.
#[test]
fn understanding_choice_without_attempt_stops_at_first_commit() {
    announce("understanding_choice_without_attempt_stops_at_first_commit");
    let mut parser = choice((string("let"), string("lex")));
    let err = parser.easy_parse(position::Stream::new("lex")).unwrap_err();
    // `string` reports a committed failure at the *start* of the string
    // (src/parser/token.rs: `from_error(start, ...)`), so the position is
    // column 1 even though the mismatch is at 'x' (column 3).
    assert_eq!(err.position, SourcePosition { line: 1, column: 1 });
    // `CommitErr` skips the error enrichment done for `PeekErr`, so the only
    // error is the unexpected token and the second branch contributes nothing.
    assert_eq!(err.errors, vec![Error::Unexpected('x'.into())]);
}

// 2) With `attempt`: the `CommitErr` of the first branch is downgraded to
//    `PeekErr`, the stream is reset to the checkpoint and the second branch
//    succeeds.
#[test]
fn understanding_choice_with_attempt_backtracks_to_checkpoint() {
    announce("understanding_choice_with_attempt_backtracks_to_checkpoint");
    let mut parser = choice((attempt(string("let")), string("lex")));
    assert_eq!(parser.parse("lex"), Ok(("lex", "")));
}

// 3) Commit in the middle: `attempt` only wraps the tail of the branch. The
//    leading `char('l')` already committed, so the merged sequence result is
//    still `CommitErr` and the second branch is not tried.
#[test]
fn understanding_commit_in_middle_still_commits() {
    announce("understanding_commit_in_middle_still_commits");
    let mut parser = choice((char('l').with(attempt(string("et"))), string("lex")));
    let err = parser.easy_parse(position::Stream::new("lex")).unwrap_err();
    // The failure is reported at the start of "et" (column 2).
    assert_eq!(err.position, SourcePosition { line: 1, column: 2 });
    assert!(err.errors.contains(&Error::Unexpected('x'.into())));
    assert!(err.errors.contains(&Error::Expected("et".into())));
    // The second branch was never attempted, so it contributes nothing.
    assert!(!err.errors.contains(&Error::Expected("lex".into())));
}

// 4) The required minimal test: after the checkpoint is restored, the reported
//    error position comes from the branch that consumed the most input (the
//    first branch, failing at column 3), not from the branch that was tried
//    last (the second branch, failing at column 1).
#[test]
fn understanding_checkpoint_reset_keeps_furthest_error_position() {
    announce("understanding_checkpoint_reset_keeps_furthest_error_position");
    let mut parser = choice((
        attempt((char('a'), char('b'), char('d'))).map(|_| ()),
        attempt(char('z')).map(|_| ()),
    ));
    let err: Errors<char, &str, SourcePosition> = parser
        .easy_parse(position::Stream::new("abc"))
        .unwrap_err();
    // Branch 1 consumed "ab" before failing on 'c' at column 3; branch 2
    // failed at column 1. `easy::Errors::merge` keeps the furthest position
    // (src/stream/easy.rs:699-717), so the position is column 3 even though
    // branch 2 was tried last (after the checkpoint reset).
    assert_eq!(err.position, SourcePosition { line: 1, column: 3 });
    assert!(err.errors.contains(&Error::Unexpected('c'.into())));
    assert!(err.errors.contains(&Error::Expected('d'.into())));
    // Note: `Expected('z')` from the last tried branch still appears, because
    // the top-level `PeekErr` handling re-adds `expected` info from *all*
    // choice branches via `add_error` (src/parser/mod.rs:210-228,
    // src/parser/choice.rs:286-298). What `merge` decides is the *position*
    // and the errors carried by the winning branch.
    assert!(err.errors.contains(&Error::Expected('z'.into())));
}

// 5) A zero-width branch always succeeds without consuming input, so any
//    later branch is unreachable.
#[test]
fn understanding_zero_width_branch_shadows_later_branches() {
    announce("understanding_zero_width_branch_shadows_later_branches");
    let mut parser = choice((
        many::<Vec<char>, _, _>(digit()).map(|v| v.len()),
        string("abc").map(|s| s.len()),
    ));
    // `many(digit())` succeeds with an empty value on "abc"; `string("abc")`
    // is never attempted and the input is left unconsumed.
    assert_eq!(parser.parse("abc"), Ok((0, "abc")));
}

// A `Read` that yields at most `chunk` bytes per call, forcing the decoder to
// cross buffer boundaries mid-token.
struct Dribble<'a> {
    data: &'a [u8],
    chunk: usize,
}

impl<'a> std::io::Read for Dribble<'a> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.chunk.min(self.data.len()).min(buf.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

// 6) Buffered reader crossing a buffer boundary: the first branch consumes
//    "he" and then hits the end of the buffer. On a partial stream that
//    end-of-buffer is a `CommitErr` (src/stream/mod.rs:246-259), so `choice`
//    records the branch in its partial state and resumes *that* branch after
//    the buffer is refilled. The second branch ("help!") is never tried even
//    though it would have matched the full input.
#[test]
fn understanding_buffered_reader_commit_survives_buffer_boundary() {
    announce("understanding_buffered_reader_commit_survives_buffer_boundary");
    use combine::stream::{Decoder, PointerOffset};

    let mut read = Dribble {
        data: &b"help!"[..],
        chunk: 2,
    };
    let mut decoder = Decoder::<_, PointerOffset<[u8]>>::new();
    let result = combine::decode!(
        decoder,
        read,
        {
            combine::choice((
                (
                    combine::parser::byte::byte(b'h'),
                    combine::parser::byte::byte(b'e'),
                    combine::parser::byte::byte(b'l'),
                    combine::parser::byte::byte(b'l'),
                    combine::parser::byte::byte(b'o'),
                )
                    .map(|_| ()),
                combine::parser::byte::bytes(&b"help!"[..]).map(|_| ()),
            ))
        },
        |input, _position| combine::easy::Stream::from(input),
    );
    let err = result
        .map_err(combine::easy::Errors::<u8, &[u8], _>::from)
        .unwrap_err();
    // The resumed first branch fails on 'p' (it expected the second 'l' of
    // "hello") ...
    assert!(err.errors.contains(&Error::Unexpected(b'p'.into())));
    assert!(err.errors.contains(&Error::Expected(b'l'.into())));
    // ... and the never-tried "help!" branch contributes no `Expected` range.
    assert!(!err
        .errors
        .iter()
        .any(|e| matches!(e, Error::Expected(Info::Range(_)))));
    // The failure points at 'p', one byte into the not-yet-consumed buffer
    // ("hel" was already committed across the boundary).
    assert_eq!(err.position.translate_position(decoder.buffer()), 1);
}
