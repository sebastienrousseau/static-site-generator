// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Element-aware rewriting of fingerprinted asset references in HTML.
//!
//! `integrity` and `crossorigin` are attributes of `<script src>` and of
//! `<link href>` with a `stylesheet`, `preload` or `modulepreload` rel,
//! and of nothing else. The previous pass rewrote the page as text and
//! appended them after every quoted asset path, so a URL inside a JSON-LD
//! graph became invalid JSON and `<meta>` and `<img>` grew attributes they
//! cannot carry (#798). Elements are now visited through `lol_html`
//! (ADR-0003): every attribute naming an asset is renamed to the
//! fingerprinted file, the SRI attributes are set on the reference that
//! carries them, and `<script>` and `<style>` bodies have quoted asset
//! paths renamed with nothing appended. Comments and text are untouched.

use super::AssetInfo;
use crate::error::SsgError;
use crate::util::html_rewriter::rewrite_html;
use lol_html::html_content::{ContentType, Element, TextChunk};
use lol_html::{element, text};
use std::cell::RefCell;
use std::collections::HashMap;

/// Rewrites asset references in `html` to their fingerprinted names and
/// adds SRI attributes where they apply.
///
/// # Errors
///
/// Returns [`SsgError::Io`] if `lol_html` fails to rewrite the document.
pub(super) fn rewrite_asset_refs(
    html: &str,
    manifest: &HashMap<String, AssetInfo>,
) -> Result<String, SsgError> {
    // Script and style cannot nest, so one buffer serves both.
    let body = RefCell::new(String::new());
    let on_body = |chunk: &mut TextChunk<'_>| {
        rewrite_raw_text(chunk, &body, manifest);
        Ok(())
    };
    rewrite_html(
        html,
        vec![
            element!("*", |el| {
                rewrite_element(el, manifest);
                Ok(())
            }),
            text!("script", on_body),
            text!("style", on_body),
        ],
    )
}

/// Renames every attribute of `el` that names an asset, then sets the
/// SRI attributes if the renamed one is the element's fetch target.
fn rewrite_element(
    el: &mut Element<'_, '_>,
    manifest: &HashMap<String, AssetInfo>,
) {
    let target = sri_target(el);
    let attrs: Vec<(String, String)> = el
        .attributes()
        .iter()
        .map(|a| (a.name(), a.value()))
        .collect();
    let mut sri = None;
    for (name, value) in attrs {
        let Some((renamed, info)) = resolve(&value, manifest) else {
            continue;
        };
        let _ = el.set_attribute(&name, &renamed);
        if target == Some(name.as_str()) {
            sri = Some(info.sri.as_str());
        }
    }
    if let Some(hash) = sri {
        // Replaces an authored `integrity`, which hashed the file before
        // it was minified; an authored `crossorigin` mode is kept.
        let _ = el.set_attribute("integrity", hash);
        if !el.has_attribute("crossorigin") {
            let _ = el.set_attribute("crossorigin", "anonymous");
        }
    }
}

/// The attribute whose resource SRI verifies on this element, if any.
fn sri_target(el: &Element<'_, '_>) -> Option<&'static str> {
    match el.tag_name().as_str() {
        "script" => Some("src"),
        "link"
            if el.get_attribute("rel").is_some_and(|rel| {
                rel.split_ascii_whitespace().any(|t| {
                    ["stylesheet", "preload", "modulepreload"]
                        .iter()
                        .any(|r| t.eq_ignore_ascii_case(r))
                })
            }) =>
        {
            Some("href")
        }
        _ => None,
    }
}

/// Buffers a `<script>` or `<style>` body and, at its last chunk, writes
/// it back with quoted asset paths renamed.
fn rewrite_raw_text(
    chunk: &mut TextChunk<'_>,
    body: &RefCell<String>,
    manifest: &HashMap<String, AssetInfo>,
) {
    let mut buf = body.borrow_mut();
    buf.push_str(chunk.as_str());
    if chunk.last_in_text_node() {
        let text = std::mem::take(&mut *buf);
        chunk.replace(&rename_quoted(&text, manifest), ContentType::Html);
    } else {
        chunk.remove();
    }
}

/// Renames each single- or double-quoted span of `text` that is exactly an
/// asset path. Nothing is appended, so JSON, JavaScript and CSS stay valid.
fn rename_quoted(text: &str, manifest: &HashMap<String, AssetInfo>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(['"', '\'']) {
        let quote = &rest[open..=open];
        out.push_str(&rest[..=open]);
        rest = &rest[open + 1..];
        let Some(close) = rest.find(quote) else { break };
        let inner = &rest[..close];
        // A span across a line break is an apostrophe pairing with a
        // later quote, not a string: rescan from just after it.
        if inner.contains('\n') {
            continue;
        }
        match resolve(inner, manifest) {
            Some((renamed, _)) => out.push_str(&renamed),
            None => out.push_str(inner),
        }
        out.push_str(quote);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The fingerprinted form of `value` if its path names a manifest asset.
///
/// The path may be the manifest key itself, or end in `/<key>`; the
/// longest matching key wins, so `/css/style.css` resolves to
/// `css/style.css` even when a root `style.css` exists too. A query or
/// fragment is carried over.
fn resolve<'m>(
    value: &str,
    manifest: &'m HashMap<String, AssetInfo>,
) -> Option<(String, &'m AssetInfo)> {
    let cut = value.find(['?', '#']).unwrap_or(value.len());
    let (path, tail) = value.split_at(cut);
    std::iter::once(0)
        .chain(path.match_indices('/').map(|(i, _)| i + 1))
        .find_map(|start| {
            manifest.get(&path[start..]).map(|info| {
                (
                    format!("{}{}{tail}", &path[..start], info.fingerprinted),
                    info,
                )
            })
        })
}

#[cfg(test)]
#[path = "html_refs_tests.rs"]
mod tests;
