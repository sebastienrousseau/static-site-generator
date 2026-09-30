// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT
#![allow(missing_docs)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! What the build log says about an invalid IBAN.
//!
//! `warn_invalid_fields` must name the page and the field and quote
//! nothing from the account: no fragment of the number and nothing
//! computed from it, such as the MOD-97 remainder. This asserts on the
//! real `log::warn!` output, captured by a logger installed for this
//! test binary alone, so it cannot race the library's other tests over
//! the global logger.

use log::{Log, Metadata, Record};
use ssg::seo::jsonld::iso20022::{
    warn_invalid_fields, BankAccount, Iso20022Entity,
};
use std::sync::Mutex;

struct Capture(Mutex<Vec<String>>);

impl Log for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &Record<'_>) {
        self.0.lock().unwrap().push(record.args().to_string());
    }
    fn flush(&self) {}
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

#[test]
fn invalid_iban_warnings_quote_nothing_from_the_account() {
    log::set_logger(&CAPTURE).expect("this binary installs no other logger");
    log::set_max_level(log::LevelFilter::Warn);

    // One IBAN per failure category; the last fails only its checksum,
    // whose reason carries the remainder.
    let accounts = [
        "GB29NWBK",               // length
        "1B29NWBK60161331926819", // country code
        "GBX9NWBK60161331926819", // check digits
        "GB29NWBK6016133192681_", // BBAN
        "GB29NWBK60161331926818", // checksum
    ];
    for number in accounts {
        let entity = Iso20022Entity::BankAccount(BankAccount {
            iban: Some(number.into()),
            ..BankAccount::default()
        });
        let _ = warn_invalid_fields(&entity, "page.md");
    }

    let lines = CAPTURE.0.lock().unwrap().clone();
    assert_eq!(lines.len(), accounts.len(), "one warning per invalid IBAN");
    for line in &lines {
        assert!(
            line.contains("page.md") && line.contains("bank_account.iban"),
            "a warning does not name the page and the field"
        );
        for fragment in ["GB29", "NWBK", "6016", "6818", "6819", "remainder"] {
            assert!(
                !line.contains(fragment),
                "a warning quotes {fragment:?}, which comes from the account"
            );
        }
    }
}
