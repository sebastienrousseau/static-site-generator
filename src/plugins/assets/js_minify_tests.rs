// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Regression tests for #799: the JavaScript minifier must copy string,
//! template and regex literals through byte for byte.

use super::minify_js;

/// Asserts `needle` survives minification of `js` unchanged, and that
/// a second pass leaves the output alone.
fn assert_survives(js: &str, needle: &str) {
    let out = minify_js(js);
    assert!(out.contains(needle), "{needle:?} altered in: {out:?}");
    assert_eq!(minify_js(&out), out, "not idempotent for: {js:?}");
}

#[test]
fn double_quoted_string_keeps_its_spaces() {
    assert_survives("var s = \"a  b\";", "\"a  b\"");
}

#[test]
fn single_quoted_trailing_space_survives() {
    assert_survives("term.prompt = '$ ' + cmd;", "'$ '");
}

#[test]
fn root_margin_string_survives() {
    let js =
        "new IntersectionObserver(cb, { rootMargin: \"0px 0px -15% 0px\" });";
    assert_survives(js, "\"0px 0px -15% 0px\"");
}

#[test]
fn template_literal_keeps_indentation_and_interpolation() {
    let tpl =
        "`\nkey:\n  nested: ${ value + 1 }\n    deeper: ${ \"`\" }  end\n`";
    assert_survives(&format!("const yaml = {tpl};"), tpl);
}

#[test]
fn nested_template_literal_survives() {
    let tpl = "`a ${ `in  ner ${ x }` } b`";
    assert_survives(&format!("let t = {tpl};"), tpl);
}

#[test]
fn regex_literal_with_spaces_and_quotes_survives() {
    assert_survives("const re = /[\"' ]+ x/g;", "/[\"' ]+ x/g");
    assert_survives("if (/ +$/.test(s)) f();", "/ +$/");
}

#[test]
fn regex_class_holding_comment_opener_survives() {
    assert_survives("const re = /[/*]/;\nconst n = 1;", "/[/*]/");
    let out = minify_js("const re = /[/*]/;\nconst n = 1;");
    assert!(out.contains("const n=1"), "{out}");
}

#[test]
fn regex_after_keyword_is_not_division() {
    assert_survives("function f(s) { return / a /.test(s); }", "/ a /");
}

#[test]
fn division_is_still_division() {
    assert_eq!(minify_js("const x = (a) / 2 / b;"), "const x=(a)/2/b;");
}

#[test]
fn comment_like_text_inside_strings_survives() {
    assert_survives("var u = \"http://x\";", "\"http://x\"");
    assert_survives("var c = '/* not a comment */';", "'/* not a comment */'");
}

#[test]
fn trailing_space_before_newline_still_separates_words() {
    assert_eq!(minify_js("let a = b \nlet c = 1;"), "let a=b\nlet c=1;");
}

#[test]
fn block_comment_between_words_separates_them() {
    assert_eq!(minify_js("return/* x */value;"), "return value;");
}

#[test]
fn unary_operators_do_not_fuse_into_increments() {
    assert_eq!(minify_js("x = a - -b + +c;"), "x=a- -b+ +c;");
}

#[test]
fn unterminated_literals_and_comments_keep_their_bytes() {
    for js in [
        "var s = \"open",
        "var s = 'open\nnext()",
        "var t = `open ${ a",
        "var t = `a ${ { b: '}' } } \\` c`",
        "var r = /open",
        "var r = /open\nnext()",
        "a /* open",
        "s = 'x\\",
    ] {
        let out = minify_js(js);
        assert_eq!(minify_js(&out), out, "not idempotent for: {js:?}");
    }
    assert_eq!(minify_js("var s = \"open"), "var s=\"open");
    assert_eq!(minify_js("var t = `open ${ a"), "var t=`open ${ a");
    assert_eq!(minify_js("var r = /open\nnext()"), "var r=/open\nnext()");
}

#[test]
fn substitution_braces_and_escapes_stay_inside_the_template() {
    let tpl = "`a ${ { b: '}', c: \"{\" }.b } \\` \\${x} d`";
    assert_survives(&format!("t = {tpl} ;"), tpl);
}

#[test]
fn line_breaks_of_every_kind_end_a_line_comment() {
    assert_eq!(minify_js("a // c\r\nb"), "a\nb");
    assert_eq!(minify_js("a /* x\n y */ b"), "a\nb");
    assert_eq!(minify_js("x = 1 .toString()"), "x=1 .toString()");
}

#[test]
fn postfix_increment_then_slash_divides() {
    assert_eq!(minify_js("y = a++ / 2 // half"), "y=a++/2");
    assert_eq!(minify_js("y = [a] / 2"), "y=[a]/2");
    assert_eq!(minify_js("y = 'n' / 2"), "y='n'/2");
}
