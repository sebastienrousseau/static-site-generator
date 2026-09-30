// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ISO 20022 / banking domain JSON-LD types.
//!
//! Opt-in extension to the [`crate::seo::JsonLdPlugin`] that lets
//! fintech / banking / payments authors emit strongly-typed, Schema.org
//! compatible JSON-LD blobs for the five most common financial domain
//! entities. Ports concepts from the ISO 20022 messaging vocabulary into
//! a Schema.org-friendly shape:
//!
//! - [`BankAccount`] — Schema.org `BankAccount` with IBAN/BIC.
//! - [`PaymentInstrument`] — card, transfer, direct debit.
//! - [`FinancialTransaction`] — extends Schema.org `MoneyTransfer`.
//! - [`RegulatedFinancialInstitution`] — extends `Organization`.
//! - [`FinancialProduct`] — loan, deposit, derivative.
//!
//! All ISO-20022-specific fields are emitted under the `iso20022:`
//! namespace prefix so that Schema.org-only validators still see a
//! conformant payload.
//!
//! # Validation
//!
//! Two validators are bundled:
//!
//! - [`validate_iban`] — performs MOD-97 checksum verification per
//!   ISO 13616.
//! - [`validate_bic`] — checks the 8/11-character ISO 9362 layout.
//!
//! Invalid values do NOT fail the build — they emit a `log::warn!`
//! naming the offending page and the field that didn't validate.
//!
//! # Documentation pointer
//!
//! On first use within a build, [`log_first_use_pointer`] emits an
//! info-level log pointing at the canonical docs URL.
//!
//! # Schema.org base mapping
//!
//! | ISO 20022 type                   | `@type`                        |
//! |----------------------------------|--------------------------------|
//! | `BankAccount`                    | `BankAccount`                  |
//! | `PaymentInstrument`              | `PaymentService`               |
//! | `FinancialTransaction`           | `MoneyTransfer`                |
//! | `RegulatedFinancialInstitution`  | `BankOrCreditUnion`            |
//! | `FinancialProduct`               | `FinancialProduct`             |

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

/// Canonical docs URL emitted on first use within a build.
pub const DOCS_URL: &str =
    "https://docs.rs/ssg/latest/ssg/seo/jsonld/iso20022/index.html";

/// Tracks whether the first-use info pointer has fired during this
/// process, so we don't spam the log for every page on a large site.
static FIRST_USE_LOGGED: AtomicBool = AtomicBool::new(false);

/// Emits an info-level log pointing at the iso20022 docs URL,
/// exactly once per process. Idempotent.
///
/// Resolves AC7.
///
/// # Examples
///
/// ```
/// use ssg::seo::jsonld::iso20022::log_first_use_pointer;
/// // Idempotent — second call is a no-op.
/// log_first_use_pointer();
/// log_first_use_pointer();
/// ```
pub fn log_first_use_pointer() {
    if !FIRST_USE_LOGGED.swap(true, Ordering::Relaxed) {
        log::info!(
            "[json-ld/iso20022] First use of ISO 20022 frontmatter detected. \
             See {DOCS_URL} for the list of available types and required fields."
        );
    }
}

#[cfg(test)]
pub(crate) fn reset_first_use_for_test() {
    FIRST_USE_LOGGED.store(false, Ordering::Relaxed);
}

#[cfg(test)]
pub(crate) fn first_use_was_logged() -> bool {
    FIRST_USE_LOGGED.load(Ordering::Relaxed)
}

// =====================================================================
// Validators
// =====================================================================

/// Result of an ISO 20022 field validation. Errors are warnings, not
/// hard failures — the build continues regardless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationOutcome {
    /// The value parsed and passed all syntactic + checksum checks.
    Valid,
    /// The value failed validation; `reason` is a human-readable note.
    Invalid {
        /// Why the value was rejected (length, checksum mismatch, etc.).
        reason: String,
    },
}

impl ValidationOutcome {
    /// Returns `true` when the outcome is [`ValidationOutcome::Valid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::seo::jsonld::iso20022::ValidationOutcome;
    /// assert!(ValidationOutcome::Valid.is_valid());
    /// let bad = ValidationOutcome::Invalid { reason: "x".into() };
    /// assert!(!bad.is_valid());
    /// ```
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }
}

/// Masks the middle of a financial identifier for logging.
///
/// # Why logging differs from publishing
///
/// An IBAN given in front matter is *meant* to be published — it ends up
/// in the emitted JSON-LD as `iso20022:iban`, because the author is
/// advertising payment details on purpose. A build log is a different
/// channel: it is captured by CI, retained in artefacts, and read over
/// shoulders. Writing a full account number there is gratuitous, and
/// `CodeQL`'s `rust/cleartext-logging` rule is right to flag it.
///
/// Enough is kept to act on the warning — the leading country and bank
/// prefix, and the trailing digits — while the account-identifying
/// middle is masked. The `reason` in the same message already says what
/// is wrong, so the author can find the value in their own front matter
/// without the log restating it.
///
/// Short inputs are masked entirely rather than partially: a 6-character
/// value split 4-and-2 would reveal most of itself.
///
/// # Examples
///
/// ```
/// use ssg::seo::jsonld::iso20022::redact_for_log;
/// assert_eq!(redact_for_log("GB29NWBK60161331926819"), "GB29…6819");
/// assert_eq!(redact_for_log("NWBKGB2L"), "………");
/// assert_eq!(redact_for_log(""), "………");
/// ```
#[must_use]
pub fn redact_for_log(value: &str) -> String {
    // Count by characters, not bytes: an operator may paste anything
    // into front matter, and slicing a multi-byte scalar would panic.
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 12 {
        return "………".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// Validates an IBAN (ISO 13616) using the MOD-97 checksum.
///
/// Accepts the canonical compact form (no spaces) as well as the
/// space-delimited print form. Length bounds: 15–34 characters
/// once whitespace is stripped.
///
/// # Algorithm
///
/// 1. Strip all ASCII whitespace; upper-case.
/// 2. Move the first 4 characters (country + check digits) to the end.
/// 3. Map letters A-Z → 10..=35.
/// 4. The resulting integer must be congruent to 1 mod 97.
///
/// Implemented without `num-bigint` by walking the digit string left
/// to right, taking each modulo step incrementally — this keeps the
/// crate dependency-free for the ISO validator.
///
/// # Examples
///
/// ```
/// use ssg::seo::jsonld::iso20022::validate_iban;
/// assert!(validate_iban("GB29NWBK60161331926819").is_valid());
/// assert!(!validate_iban("GB29").is_valid());
/// ```
#[must_use]
pub fn validate_iban(input: &str) -> ValidationOutcome {
    match check_iban(input) {
        Ok(()) => ValidationOutcome::Valid,
        Err(fault) => ValidationOutcome::Invalid {
            reason: fault.reason(),
        },
    }
}

/// The first failure [`check_iban`] found, with the measurements behind
/// [`validate_iban`]'s detailed reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IbanFault {
    Length(usize),
    CountryCode,
    CheckDigits,
    Bban,
    Character { position: usize },
    Checksum { remainder: u64 },
}

impl IbanFault {
    /// The detailed reason for callers of [`validate_iban`]; never logged.
    fn reason(self) -> String {
        match self {
            Self::Length(len) => {
                format!("IBAN length {len} outside ISO 13616 range 15..=34")
            }
            Self::CountryCode => {
                "IBAN country code must be two ASCII letters".to_string()
            }
            Self::CheckDigits => {
                "IBAN check digits must be two ASCII digits".to_string()
            }
            Self::Bban => "IBAN BBAN must be alphanumeric ASCII".to_string(),
            Self::Character { position } => {
                format!(
                    "Non-alphanumeric character in IBAN at position {position}"
                )
            }
            Self::Checksum { remainder } => {
                format!("IBAN MOD-97 checksum failed (remainder={remainder})")
            }
        }
    }
}

/// The checks behind [`validate_iban`], returning the first failure.
fn check_iban(input: &str) -> Result<(), IbanFault> {
    let compact: String = input
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();

    if compact.len() < 15 || compact.len() > 34 {
        return Err(IbanFault::Length(compact.len()));
    }

    // First two chars must be ASCII letters (country code), next two ASCII digits (check digits).
    let bytes = compact.as_bytes();
    if !(bytes[0].is_ascii_alphabetic() && bytes[1].is_ascii_alphabetic()) {
        return Err(IbanFault::CountryCode);
    }
    if !(bytes[2].is_ascii_digit() && bytes[3].is_ascii_digit()) {
        return Err(IbanFault::CheckDigits);
    }
    // The remaining BBAN must be alphanumeric.
    if !bytes[4..].iter().all(u8::is_ascii_alphanumeric) {
        return Err(IbanFault::Bban);
    }

    // Rearrange: move first 4 chars to end.
    let mut rearranged = String::with_capacity(compact.len());
    rearranged.push_str(&compact[4..]);
    rearranged.push_str(&compact[..4]);

    // Expand letters → numeric (A=10..Z=35) and compute mod 97 streaming.
    let mut remainder: u64 = 0;
    for (position, ch) in rearranged.chars().enumerate() {
        let digits: u64 = if ch.is_ascii_digit() {
            u64::from(ch as u8 - b'0')
        } else if ch.is_ascii_alphabetic() {
            u64::from((ch.to_ascii_uppercase() as u8) - b'A') + 10
        } else {
            // The offending character is deliberately not echoed. This
            // reason reaches `log::warn!` in `warn_invalid_fields`, which
            // redacts the IBAN itself -- and then quoted one character of
            // it straight back into the same line, which is what
            // `rust/cleartext-logging` flagged. A position is enough to
            // locate the problem and reveals nothing about the account.
            return Err(IbanFault::Character { position });
        };
        // Each letter expands to two digits (10..=35); fold accordingly.
        if digits >= 10 {
            remainder = (remainder * 100 + digits) % 97;
        } else {
            remainder = (remainder * 10 + digits) % 97;
        }
    }

    if remainder == 1 {
        Ok(())
    } else {
        Err(IbanFault::Checksum { remainder })
    }
}

/// Validates a BIC (ISO 9362) by length + alphanumeric layout.
///
/// Valid BICs are 8 or 11 characters, all ASCII letters and digits.
/// The first 4 are the bank code (letters), next 2 the country code
/// (letters), next 2 the location code (alphanumeric); positions 9–11
/// (when present) are the branch code.
///
/// # Examples
///
/// ```
/// use ssg::seo::jsonld::iso20022::validate_bic;
/// assert!(validate_bic("NWBKGB2L").is_valid());
/// assert!(validate_bic("NWBKGB2LXXX").is_valid());
/// assert!(!validate_bic("NWBKGB").is_valid());
/// ```
#[must_use]
pub fn validate_bic(input: &str) -> ValidationOutcome {
    let compact: String =
        input.chars().filter(|c| !c.is_whitespace()).collect();

    if compact.len() != 8 && compact.len() != 11 {
        return ValidationOutcome::Invalid {
            reason: format!(
                "BIC length {} is not 8 or 11 (ISO 9362)",
                compact.len()
            ),
        };
    }
    let upper = compact.to_ascii_uppercase();
    let bytes = upper.as_bytes();

    // Bank code: 4 letters
    if !bytes[..4].iter().all(u8::is_ascii_alphabetic) {
        return ValidationOutcome::Invalid {
            reason: "BIC bank code (chars 1-4) must be ASCII letters"
                .to_string(),
        };
    }
    // Country code: 2 letters
    if !bytes[4..6].iter().all(u8::is_ascii_alphabetic) {
        return ValidationOutcome::Invalid {
            reason: "BIC country code (chars 5-6) must be ASCII letters"
                .to_string(),
        };
    }
    // Location code: 2 alphanumerics
    if !bytes[6..8].iter().all(u8::is_ascii_alphanumeric) {
        return ValidationOutcome::Invalid {
            reason: "BIC location code (chars 7-8) must be ASCII alphanumerics"
                .to_string(),
        };
    }
    // Optional branch code: 3 alphanumerics
    if compact.len() == 11
        && !bytes[8..11].iter().all(u8::is_ascii_alphanumeric)
    {
        return ValidationOutcome::Invalid {
            reason: "BIC branch code (chars 9-11) must be ASCII alphanumerics"
                .to_string(),
        };
    }

    ValidationOutcome::Valid
}

// =====================================================================
// Domain types
// =====================================================================

/// ISO 4217 monetary amount.
///
/// Skips serialisation when both fields are empty/zero — keeps the
/// JSON-LD compact when authors leave the amount out of frontmatter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonetaryAmount {
    /// ISO 4217 currency code (e.g. `EUR`, `USD`).
    pub currency: String,
    /// Numeric amount.
    pub amount: f64,
}

impl MonetaryAmount {
    /// Renders to a Schema.org `MonetaryAmount` JSON value.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::seo::jsonld::iso20022::MonetaryAmount;
    /// let m = MonetaryAmount { currency: "EUR".into(), amount: 100.0 };
    /// let v = m.to_jsonld();
    /// assert_eq!(v["currency"], "EUR");
    /// assert_eq!(v["value"], 100.0);
    /// ```
    #[must_use]
    pub fn to_jsonld(&self) -> serde_json::Value {
        serde_json::json!({
            "@type": "MonetaryAmount",
            "currency": self.currency,
            "value": self.amount,
        })
    }
}

/// A bank account record. Schema.org base type: `BankAccount`.
///
/// IBAN/BIC are optional — but if either is supplied, they are validated
/// and warnings emitted on mismatch. The IBAN ends up under the
/// `iso20022:iban` namespaced field; BIC similarly under `iso20022:bic`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BankAccount {
    /// Account holder display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// International Bank Account Number (ISO 13616).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iban: Option<String>,
    /// Bank Identifier Code (ISO 9362, 8 or 11 chars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bic: Option<String>,
}

impl BankAccount {
    /// Builds the JSON-LD blob for this account.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::seo::jsonld::iso20022::BankAccount;
    /// let acc = BankAccount {
    ///     name: Some("Treasury".into()),
    ///     iban: Some("GB29NWBK60161331926819".into()),
    ///     bic: None,
    /// };
    /// let v = acc.to_jsonld();
    /// assert_eq!(v["@type"], "BankAccount");
    /// assert_eq!(v["iso20022:iban"], "GB29NWBK60161331926819");
    /// ```
    #[must_use]
    pub fn to_jsonld(&self) -> serde_json::Value {
        let mut obj = serde_json::json!({
            "@context": context_with_iso(),
            "@type": "BankAccount",
        });
        if let Some(name) = &self.name {
            obj["name"] = serde_json::json!(name);
        }
        if let Some(iban) = &self.iban {
            // The IBAN is already the typed identifier via the
            // `iso20022:iban` namespace; duplicating it into the
            // generic schema.org `identifier` field would publish the
            // same string twice and trip downstream consumers that
            // expect distinct values across fields.
            obj["iso20022:iban"] = serde_json::json!(iban);
        }
        if let Some(bic) = &self.bic {
            obj["iso20022:bic"] = serde_json::json!(bic);
        }
        obj
    }
}

/// A payment instrument (card, transfer, direct debit). Schema.org
/// base type: `PaymentService`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PaymentInstrument {
    /// Human-readable name (e.g. "Visa Debit").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Instrument family. Accepted values: `card`, `transfer`,
    /// `direct_debit` — anything else passes through.
    pub instrument_type: String,
    /// Optional brand string (e.g. "Visa", "SEPA").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
}

impl PaymentInstrument {
    /// Builds the JSON-LD blob for this instrument.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::seo::jsonld::iso20022::PaymentInstrument;
    /// let p = PaymentInstrument {
    ///     name: Some("Visa Debit".into()),
    ///     instrument_type: "card".into(),
    ///     brand: Some("Visa".into()),
    /// };
    /// let v = p.to_jsonld();
    /// assert_eq!(v["@type"], "PaymentService");
    /// assert_eq!(v["iso20022:instrumentType"], "card");
    /// ```
    #[must_use]
    pub fn to_jsonld(&self) -> serde_json::Value {
        let mut obj = serde_json::json!({
            "@context": context_with_iso(),
            "@type": "PaymentService",
            "iso20022:instrumentType": self.instrument_type,
        });
        if let Some(name) = &self.name {
            obj["name"] = serde_json::json!(name);
        }
        if let Some(brand) = &self.brand {
            obj["brand"] = serde_json::json!(brand);
        }
        obj
    }
}

/// A financial transaction. Schema.org base type: `MoneyTransfer`.
///
/// Authors typically supply `instructed_amount` plus debtor/creditor
/// accounts; the resulting JSON-LD exposes a Schema.org-shaped
/// `amount` field alongside the namespaced `iso20022:*Account`
/// references.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FinancialTransaction {
    /// Amount being transferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructed_amount: Option<MonetaryAmount>,
    /// Account being debited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debtor_account: Option<BankAccount>,
    /// Account being credited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creditor_account: Option<BankAccount>,
    /// ISO 8601 timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_date: Option<String>,
    /// Optional unique identifier (`EndToEndId` / `MessageId`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_to_end_id: Option<String>,
}

impl FinancialTransaction {
    /// Builds the JSON-LD blob for this transaction.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::seo::jsonld::iso20022::{FinancialTransaction, MonetaryAmount};
    /// let tx = FinancialTransaction {
    ///     instructed_amount: Some(MonetaryAmount {
    ///         currency: "EUR".into(),
    ///         amount: 50.0,
    ///     }),
    ///     ..FinancialTransaction::default()
    /// };
    /// let v = tx.to_jsonld();
    /// assert_eq!(v["@type"], "MoneyTransfer");
    /// assert_eq!(v["amount"]["currency"], "EUR");
    /// ```
    #[must_use]
    pub fn to_jsonld(&self) -> serde_json::Value {
        let mut obj = serde_json::json!({
            "@context": context_with_iso(),
            "@type": "MoneyTransfer",
        });
        if let Some(amount) = &self.instructed_amount {
            obj["amount"] = amount.to_jsonld();
        }
        if let Some(debtor) = &self.debtor_account {
            obj["iso20022:debtorAccount"] = strip_context(debtor.to_jsonld());
        }
        if let Some(creditor) = &self.creditor_account {
            obj["iso20022:creditorAccount"] =
                strip_context(creditor.to_jsonld());
        }
        if let Some(date) = &self.execution_date {
            obj["iso20022:executionDate"] = serde_json::json!(date);
        }
        if let Some(id) = &self.end_to_end_id {
            obj["iso20022:endToEndId"] = serde_json::json!(id);
            obj["identifier"] = serde_json::json!(id);
        }
        obj
    }
}

/// A regulated financial institution. Schema.org base type:
/// `BankOrCreditUnion`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RegulatedFinancialInstitution {
    /// Display name of the institution.
    pub name: String,
    /// Optional Legal Entity Identifier (ISO 17442).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lei: Option<String>,
    /// Regulator's licence reference (e.g. UK FCA FRN).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence_id: Option<String>,
    /// Regulator name (e.g. "FCA", "`BaFin`", "ECB").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regulator: Option<String>,
    /// Optional canonical URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl RegulatedFinancialInstitution {
    /// Builds the JSON-LD blob for this institution.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::seo::jsonld::iso20022::RegulatedFinancialInstitution;
    /// let r = RegulatedFinancialInstitution {
    ///     name: "Acme Bank".into(),
    ///     ..RegulatedFinancialInstitution::default()
    /// };
    /// let v = r.to_jsonld();
    /// assert_eq!(v["@type"], "BankOrCreditUnion");
    /// assert_eq!(v["name"], "Acme Bank");
    /// ```
    #[must_use]
    pub fn to_jsonld(&self) -> serde_json::Value {
        let mut obj = serde_json::json!({
            "@context": context_with_iso(),
            "@type": "BankOrCreditUnion",
            "name": self.name,
        });
        if let Some(lei) = &self.lei {
            obj["iso20022:lei"] = serde_json::json!(lei);
            obj["identifier"] = serde_json::json!(lei);
        }
        if let Some(licence) = &self.licence_id {
            obj["iso20022:licenceId"] = serde_json::json!(licence);
        }
        if let Some(regulator) = &self.regulator {
            obj["iso20022:regulator"] = serde_json::json!(regulator);
        }
        if let Some(url) = &self.url {
            obj["url"] = serde_json::json!(url);
        }
        obj
    }
}

/// A financial product (loan, deposit, derivative). Schema.org base
/// type: `FinancialProduct`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FinancialProduct {
    /// Product display name.
    pub name: String,
    /// Product category. Accepted values: `loan`, `deposit`,
    /// `derivative` — anything else passes through.
    pub product_type: String,
    /// Optional issuing institution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// Optional annual percentage rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annual_percentage_rate: Option<f64>,
    /// Optional ISIN (ISO 6166).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isin: Option<String>,
}

impl FinancialProduct {
    /// Builds the JSON-LD blob for this product.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::seo::jsonld::iso20022::FinancialProduct;
    /// let p = FinancialProduct {
    ///     name: "Green Bond".into(),
    ///     product_type: "deposit".into(),
    ///     ..FinancialProduct::default()
    /// };
    /// let v = p.to_jsonld();
    /// assert_eq!(v["@type"], "FinancialProduct");
    /// assert_eq!(v["iso20022:productType"], "deposit");
    /// ```
    #[must_use]
    pub fn to_jsonld(&self) -> serde_json::Value {
        let mut obj = serde_json::json!({
            "@context": context_with_iso(),
            "@type": "FinancialProduct",
            "name": self.name,
            "iso20022:productType": self.product_type,
        });
        if let Some(issuer) = &self.issuer {
            obj["provider"] = serde_json::json!({
                "@type": "Organization",
                "name": issuer,
            });
        }
        if let Some(apr) = self.annual_percentage_rate {
            obj["annualPercentageRate"] = serde_json::json!(apr);
        }
        if let Some(isin) = &self.isin {
            obj["iso20022:isin"] = serde_json::json!(isin);
            obj["identifier"] = serde_json::json!(isin);
        }
        obj
    }
}

/// Builds the `@context` object that pulls in the `iso20022:` prefix
/// alongside the Schema.org base context.
fn context_with_iso() -> serde_json::Value {
    serde_json::json!({
        "@vocab": "https://schema.org/",
        "iso20022": "https://www.iso20022.org/",
    })
}

/// Strips the `@context` field from a nested JSON-LD value — used when
/// embedding a sub-entity (e.g. an account inside a transaction) so
/// the context only appears once at the top level.
fn strip_context(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = value.as_object_mut() {
        let _ = obj.remove("@context");
    }
    value
}

// =====================================================================
// Frontmatter dispatch
// =====================================================================

/// Tagged union dispatched from the `iso20022.type` frontmatter key.
#[derive(Debug, Clone, PartialEq)]
pub enum Iso20022Entity {
    /// Bank account variant — see [`BankAccount`].
    BankAccount(BankAccount),
    /// Payment instrument variant — see [`PaymentInstrument`].
    PaymentInstrument(PaymentInstrument),
    /// Financial transaction variant — see [`FinancialTransaction`].
    FinancialTransaction(FinancialTransaction),
    /// Regulated financial institution variant — see [`RegulatedFinancialInstitution`].
    RegulatedFinancialInstitution(RegulatedFinancialInstitution),
    /// Financial product variant — see [`FinancialProduct`].
    FinancialProduct(FinancialProduct),
}

impl Iso20022Entity {
    /// Renders this entity to a JSON-LD blob.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::seo::jsonld::iso20022::{BankAccount, Iso20022Entity};
    /// let e = Iso20022Entity::BankAccount(BankAccount::default());
    /// let v = e.to_jsonld();
    /// assert_eq!(v["@type"], "BankAccount");
    /// ```
    #[must_use]
    pub fn to_jsonld(&self) -> serde_json::Value {
        match self {
            Self::BankAccount(b) => b.to_jsonld(),
            Self::PaymentInstrument(p) => p.to_jsonld(),
            Self::FinancialTransaction(t) => t.to_jsonld(),
            Self::RegulatedFinancialInstitution(r) => r.to_jsonld(),
            Self::FinancialProduct(p) => p.to_jsonld(),
        }
    }

    /// Returns the discriminant string (e.g. `"BankAccount"`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ssg::seo::jsonld::iso20022::{BankAccount, Iso20022Entity};
    /// let e = Iso20022Entity::BankAccount(BankAccount::default());
    /// assert_eq!(e.type_name(), "BankAccount");
    /// ```
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::BankAccount(_) => "BankAccount",
            Self::PaymentInstrument(_) => "PaymentInstrument",
            Self::FinancialTransaction(_) => "FinancialTransaction",
            Self::RegulatedFinancialInstitution(_) => {
                "RegulatedFinancialInstitution"
            }
            Self::FinancialProduct(_) => "FinancialProduct",
        }
    }
}

/// Errors that can occur when interpreting an `iso20022:` frontmatter
/// block. These do NOT abort the build — they cause the block to be
/// skipped with a warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    /// The frontmatter `iso20022.type` field was missing.
    MissingType,
    /// The `type` field referenced an unknown discriminant.
    UnknownType(String),
    /// The payload failed to deserialise into the requested type.
    Malformed(String),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingType => write!(
                f,
                "iso20022 frontmatter is missing the required `type` field"
            ),
            Self::UnknownType(t) => write!(
                f,
                "iso20022 frontmatter type `{t}` is not one of: \
                 BankAccount, PaymentInstrument, FinancialTransaction, \
                 RegulatedFinancialInstitution, FinancialProduct"
            ),
            Self::Malformed(reason) => {
                write!(f, "iso20022 frontmatter payload is malformed: {reason}")
            }
        }
    }
}

/// Parses the `iso20022:` frontmatter block into a typed entity.
///
/// Returns `Err(DispatchError)` if the discriminant is missing or
/// unknown — caller decides whether to warn or fail.
///
/// # Errors
///
/// Returns [`DispatchError::MissingType`] when no `type` discriminant
/// is present, [`DispatchError::UnknownType`] when the discriminant
/// is unrecognised, or [`DispatchError::Malformed`] when the payload
/// fails to deserialise.
///
/// # Examples
///
/// ```
/// use ssg::seo::jsonld::iso20022::from_frontmatter;
/// let fm = serde_json::json!({
///     "type": "BankAccount",
///     "iban": "GB29NWBK60161331926819",
/// });
/// let entity = from_frontmatter(&fm).unwrap();
/// assert_eq!(entity.type_name(), "BankAccount");
/// ```
pub fn from_frontmatter(
    value: &serde_json::Value,
) -> Result<Iso20022Entity, DispatchError> {
    let type_str = value
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or(DispatchError::MissingType)?;

    // Clone-and-strip the `type` field so the inner shape matches the
    // struct definition cleanly.
    let mut payload = value.clone();
    if let Some(obj) = payload.as_object_mut() {
        let _ = obj.remove("type");
    }

    match type_str {
        "BankAccount" => serde_json::from_value::<BankAccount>(payload)
            .map(Iso20022Entity::BankAccount)
            .map_err(|e| DispatchError::Malformed(e.to_string())),
        "PaymentInstrument" => {
            serde_json::from_value::<PaymentInstrument>(payload)
                .map(Iso20022Entity::PaymentInstrument)
                .map_err(|e| DispatchError::Malformed(e.to_string()))
        }
        "FinancialTransaction" => {
            serde_json::from_value::<FinancialTransaction>(payload)
                .map(Iso20022Entity::FinancialTransaction)
                .map_err(|e| DispatchError::Malformed(e.to_string()))
        }
        "RegulatedFinancialInstitution" => {
            serde_json::from_value::<RegulatedFinancialInstitution>(payload)
                .map(Iso20022Entity::RegulatedFinancialInstitution)
                .map_err(|e| DispatchError::Malformed(e.to_string()))
        }
        "FinancialProduct" => {
            serde_json::from_value::<FinancialProduct>(payload)
                .map(Iso20022Entity::FinancialProduct)
                .map_err(|e| DispatchError::Malformed(e.to_string()))
        }
        other => Err(DispatchError::UnknownType(other.to_string())),
    }
}

/// Walks every IBAN/BIC inside an entity and emits `log::warn!` for
/// anything that fails validation. The `page_label` is included in the
/// warning so site authors can locate the offending page.
///
/// Returns the count of validation warnings emitted — useful for
/// asserting in tests.
///
/// # Examples
///
/// ```
/// use ssg::seo::jsonld::iso20022::{BankAccount, Iso20022Entity, warn_invalid_fields};
/// let e = Iso20022Entity::BankAccount(BankAccount {
///     iban: Some("INVALID".into()),
///     ..BankAccount::default()
/// });
/// assert_eq!(warn_invalid_fields(&e, "page.md"), 1);
/// ```
pub fn warn_invalid_fields(entity: &Iso20022Entity, page_label: &str) -> usize {
    // Each arm logs a literal: the account decides which line prints,
    // but nothing read or computed from it is interpolated, so there is
    // no data flow from the IBAN to the log (`rust/cleartext-logging`).
    // Only the page and the field are named.
    fn warn_iban(page_label: &str, iban: &str, who: &str) -> usize {
        let Err(fault) = check_iban(iban) else {
            return 0;
        };
        match fault {
            IbanFault::Length(_) => log::warn!(
                "[json-ld/iso20022] {page_label}: invalid IBAN on {who}: \
                 length outside the ISO 13616 range 15..=34"
            ),
            IbanFault::CountryCode => log::warn!(
                "[json-ld/iso20022] {page_label}: invalid IBAN on {who}: \
                 country code is not two ASCII letters"
            ),
            IbanFault::CheckDigits => log::warn!(
                "[json-ld/iso20022] {page_label}: invalid IBAN on {who}: \
                 check digits are not two ASCII digits"
            ),
            IbanFault::Bban => log::warn!(
                "[json-ld/iso20022] {page_label}: invalid IBAN on {who}: \
                 BBAN is not alphanumeric ASCII"
            ),
            IbanFault::Character { .. } => log::warn!(
                "[json-ld/iso20022] {page_label}: invalid IBAN on {who}: \
                 contains a non-alphanumeric character"
            ),
            IbanFault::Checksum { .. } => log::warn!(
                "[json-ld/iso20022] {page_label}: invalid IBAN on {who}: \
                 MOD-97 checksum failed"
            ),
        }
        1
    }
    fn warn_bic(page_label: &str, bic: &str, who: &str) -> usize {
        if let ValidationOutcome::Invalid { reason } = validate_bic(bic) {
            log::warn!(
                "[json-ld/iso20022] {page_label}: invalid BIC on {who}: \
                 {} — {reason}",
                redact_for_log(bic)
            );
            1
        } else {
            0
        }
    }

    let mut warnings = 0_usize;
    match entity {
        Iso20022Entity::BankAccount(b) => {
            if let Some(iban) = &b.iban {
                warnings += warn_iban(page_label, iban, "bank_account.iban");
            }
            if let Some(bic) = &b.bic {
                warnings += warn_bic(page_label, bic, "bank_account.bic");
            }
        }
        Iso20022Entity::FinancialTransaction(t) => {
            if let Some(d) = &t.debtor_account {
                if let Some(iban) = &d.iban {
                    warnings +=
                        warn_iban(page_label, iban, "debtor_account.iban");
                }
                if let Some(bic) = &d.bic {
                    warnings += warn_bic(page_label, bic, "debtor_account.bic");
                }
            }
            if let Some(c) = &t.creditor_account {
                if let Some(iban) = &c.iban {
                    warnings +=
                        warn_iban(page_label, iban, "creditor_account.iban");
                }
                if let Some(bic) = &c.bic {
                    warnings +=
                        warn_bic(page_label, bic, "creditor_account.bic");
                }
            }
        }
        Iso20022Entity::RegulatedFinancialInstitution(_)
        | Iso20022Entity::PaymentInstrument(_)
        | Iso20022Entity::FinancialProduct(_) => {}
    }

    warnings
}

// =====================================================================
// Minimal Schema.org JSON-LD validator
// =====================================================================

/// Validation error raised by the bundled Schema.org subset validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaOrgError {
    /// The `@type` (or "Unknown") whose required field tripped.
    pub schema_type: String,
    /// The missing or wrong-shape field.
    pub field: String,
    /// Human-readable reason.
    pub reason: String,
}

/// Validates an ISO 20022 JSON-LD blob against the Schema.org subset.
///
/// Checks (per `@type`):
/// - `@context` is present and references `schema.org`.
/// - `BankAccount`: no hard required fields, but at least one
///   identifier (`identifier` / `iso20022:iban` / `iso20022:bic`) must
///   be set so search engines have something to dedupe against.
/// - `MoneyTransfer`: requires `amount` of shape `MonetaryAmount`.
/// - `BankOrCreditUnion`: requires `name`.
/// - `FinancialProduct`: requires `name`.
/// - `PaymentService`: requires the namespaced `iso20022:instrumentType`.
///
/// This is a deliberately small subset of the Schema.org vocabulary —
/// only the fields that affect downstream rich-result indexing.
///
/// # Examples
///
/// ```
/// use ssg::seo::jsonld::iso20022::{BankAccount, validate_schema_org};
/// let v = BankAccount {
///     iban: Some("GB29NWBK60161331926819".into()),
///     ..BankAccount::default()
/// }.to_jsonld();
/// assert!(validate_schema_org(&v).is_empty());
/// ```
#[must_use]
#[allow(clippy::collapsible_match)]
pub fn validate_schema_org(value: &serde_json::Value) -> Vec<SchemaOrgError> {
    let mut errors = Vec::new();

    let schema_type = value
        .get("@type")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    // Verify @context references schema.org.
    let ctx_ok = value.get("@context").is_some_and(|c| {
        let s = serde_json::to_string(c).unwrap_or_default();
        s.contains("schema.org")
    });
    if !ctx_ok {
        errors.push(SchemaOrgError {
            schema_type: schema_type.clone(),
            field: "@context".to_string(),
            reason: "missing or does not reference schema.org".to_string(),
        });
    }

    {
        match schema_type.as_str() {
            "BankAccount" => {
                let has_id = ["identifier", "iso20022:iban", "iso20022:bic"]
                    .iter()
                    .any(|f| value.get(*f).is_some());
                if !has_id {
                    errors.push(SchemaOrgError {
                        schema_type,
                        field: "identifier|iso20022:iban|iso20022:bic"
                            .to_string(),
                        reason:
                            "BankAccount must carry at least one identifier"
                                .to_string(),
                    });
                }
            }
            "MoneyTransfer" => {
                let amount = value.get("amount");
                let amount_ok = amount.is_some_and(|a| {
                    a.get("@type").and_then(|t| t.as_str())
                        == Some("MonetaryAmount")
                        && a.get("currency").is_some()
                        && a.get("value").is_some()
                });
                if !amount_ok {
                    errors.push(SchemaOrgError {
                        schema_type,
                        field: "amount".to_string(),
                        reason: "MoneyTransfer.amount must be a \
                                 MonetaryAmount with currency and value"
                            .to_string(),
                    });
                }
            }
            "BankOrCreditUnion" | "FinancialProduct" => {
                let missing = value
                    .get("name")
                    .and_then(|v| v.as_str())
                    .is_none_or(str::is_empty);
                if missing {
                    errors.push(SchemaOrgError {
                        schema_type,
                        field: "name".to_string(),
                        reason: "required field is missing or empty"
                            .to_string(),
                    });
                }
            }
            "PaymentService" => {
                let missing = value.get("iso20022:instrumentType").is_none();
                if missing {
                    errors.push(SchemaOrgError {
                        schema_type,
                        field: "iso20022:instrumentType".to_string(),
                        reason: "PaymentService requires the namespaced \
                                 instrumentType field"
                            .to_string(),
                    });
                }
            }
            _ => {}
        }
    }

    errors
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {

    #[test]
    fn warn_invalid_fields_counts_each_iban_fault_once() {
        for bad in [
            "GB29",                   // length
            "1B29NWBK60161331926819", // country code
            "GBX9NWBK60161331926819", // check digits
            "GB29NWBK6016133192681_", // BBAN
            "GB29NWBK60161331926818", // checksum
        ] {
            let e = Iso20022Entity::BankAccount(BankAccount {
                iban: Some(bad.into()),
                ..BankAccount::default()
            });
            assert_eq!(warn_invalid_fields(&e, "page.md"), 1, "{bad}");
        }
        let ok = Iso20022Entity::BankAccount(BankAccount {
            iban: Some("GB29NWBK60161331926819".into()),
            ..BankAccount::default()
        });
        assert_eq!(warn_invalid_fields(&ok, "page.md"), 0);
    }

    /// A validation reason must never quote the value it rejected.
    ///
    /// These reasons are logged. `warn_invalid_fields` redacts the IBAN
    /// before logging it and then interpolated the reason into the same
    /// line -- and one reason quoted a character of the account number
    /// straight back, which is what `rust/cleartext-logging` flagged.
    /// Redacting a value in one half of a log line and echoing part of it
    /// in the other half is not redaction.
    ///
    /// Asserted over distinctive inputs so a future reason that starts
    /// interpolating the value fails here rather than in a scan weeks
    /// later.
    ///
    /// Scope, stated because it is easy to overclaim: this covers the
    /// reasons `validate_iban` can actually produce. The MOD-97 loop's
    /// non-alphanumeric branch -- the line `CodeQL` pointed at -- is
    /// unreachable, because the BBAN check above it already rejects any
    /// non-alphanumeric character and the first four are validated
    /// separately. Removing the redaction there does not fail this test,
    /// which was verified rather than assumed. It is fixed anyway: the
    /// branch is one reordering away from being live.
    #[test]
    fn validation_reasons_never_quote_the_rejected_value() {
        let cases = [
            "GB82!WEST12345698765432",
            "GB82 WEST 1234 5698 7654 32£",
            "ZZ99QQQQ00000000000000",
            "GB82WEST1234569876543Z2",
        ];

        for (case_idx, raw) in cases.iter().enumerate() {
            if let ValidationOutcome::Invalid { reason } = validate_iban(raw) {
                // Every run of 4+ alphanumerics from the input must be
                // absent from the reason.
                for chunk in raw
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .filter(|c| c.len() >= 4)
                {
                    // The assertion message deliberately does not echo
                    // `reason` or the input: doing so flowed the rejected
                    // value into a panic message, which CodeQL correctly
                    // reports as cleartext logging — the exact pattern this
                    // test exists to forbid. Case index locates a failure.
                    assert!(
                        !reason.contains(chunk),
                        "validation reason quotes a run of the rejected \
                         input (case {case_idx})"
                    );
                }
                // And no non-alphanumeric character from the input either.
                for ch in raw.chars().filter(|c| {
                    !c.is_ascii_alphanumeric() && !c.is_whitespace()
                }) {
                    assert!(
                        !reason.contains(ch),
                        "validation reason quotes a character of the \
                         rejected input (case {case_idx})"
                    );
                }
            }
        }
    }

    use super::*;

    // ── Validators ──────────────────────────────────────────────────

    #[test]
    fn iban_uk_natwest_valid() {
        // Known good IBAN from ISO test vectors.
        assert!(validate_iban("GB29NWBK60161331926819").is_valid());
    }

    #[test]
    fn iban_de_deutsche_valid() {
        assert!(validate_iban("DE89370400440532013000").is_valid());
    }

    #[test]
    fn iban_accepts_space_print_form() {
        assert!(validate_iban("GB29 NWBK 6016 1331 9268 19").is_valid());
    }

    /// Extracts the rejection reason, or the empty string for a
    /// [`ValidationOutcome::Valid`] result.
    fn invalid_reason(outcome: ValidationOutcome) -> String {
        match outcome {
            ValidationOutcome::Invalid { reason } => reason,
            ValidationOutcome::Valid => String::new(),
        }
    }

    #[test]
    fn invalid_reason_is_empty_for_valid_outcome() {
        assert_eq!(invalid_reason(ValidationOutcome::Valid), "");
    }

    #[test]
    fn iban_rejects_bad_checksum() {
        // Tweak last digit so MOD-97 fails.
        let res = validate_iban("GB29NWBK60161331926811");
        assert!(!res.is_valid());
        assert!(invalid_reason(res).contains("MOD-97"));
    }

    #[test]
    fn iban_rejects_short_input() {
        assert!(!validate_iban("GB29").is_valid());
    }

    #[test]
    fn iban_rejects_non_letter_country_code() {
        let res = validate_iban("1229NWBK60161331926819");
        assert!(!res.is_valid());
    }

    #[test]
    fn bic_8_chars_valid() {
        assert!(validate_bic("NWBKGB2L").is_valid());
    }

    #[test]
    fn bic_11_chars_valid() {
        assert!(validate_bic("NWBKGB2LXXX").is_valid());
    }

    #[test]
    fn bic_rejects_9_char_length() {
        let res = validate_bic("NWBKGB2LX");
        assert!(!res.is_valid());
        assert!(invalid_reason(res).contains("8 or 11"));
    }

    #[test]
    fn bic_rejects_digit_in_country_code() {
        // Pos 5-6 must be letters.
        assert!(!validate_bic("NWBK12 2L").is_valid());
    }

    #[test]
    fn iban_rejects_non_alphanumeric_bban() {
        // Covers lines 180-182: BBAN char not alphanumeric.
        let res = validate_iban("GB29NWBK60161331926*1");
        assert!(matches!(
            &res,
            ValidationOutcome::Invalid { reason } if reason.contains("BBAN")
        ));
    }

    #[test]
    fn iban_rejects_bad_check_digits() {
        // Pos 3-4 must be digits — letter there is a 173-176 hit.
        let res = validate_iban("GBABNWBK60161331926819");
        assert!(!res.is_valid());
    }

    #[test]
    fn bic_rejects_digit_in_bank_code() {
        // Covers lines 254-257: chars 1-4 must be letters.
        let res = validate_bic("1WBKGB2L");
        assert!(matches!(
            &res,
            ValidationOutcome::Invalid { reason } if reason.contains("bank code")
        ));
    }

    #[test]
    fn bic_rejects_letter_in_location_code() {
        // Covers lines 267-271: chars 7-8 must be alphanumeric.
        // (Using a non-ASCII or punctuation char triggers the
        //  alphanumeric rule; we rely on length checks first then
        //  alphabet rules.) A space-stripped non-ascii will be
        //  filtered out though; so use a punctuation symbol that
        //  isn't whitespace.
        let res = validate_bic("NWBKGB!!");
        assert!(!res.is_valid());
    }

    #[test]
    fn bic_rejects_non_alphanumeric_branch_code() {
        // Covers lines 277-280: 11-char BIC with bad branch code.
        let res = validate_bic("NWBKGB2L!!!");
        assert!(!res.is_valid());
    }

    #[test]
    fn entity_type_name_covers_all_variants() {
        // Covers the RegulatedFinancialInstitution + FinancialProduct
        // match arms at lines 695-698.
        let rfi = Iso20022Entity::RegulatedFinancialInstitution(
            RegulatedFinancialInstitution::default(),
        );
        assert_eq!(rfi.type_name(), "RegulatedFinancialInstitution");

        let fp = Iso20022Entity::FinancialProduct(FinancialProduct::default());
        assert_eq!(fp.type_name(), "FinancialProduct");
    }

    // ── Domain → JSON-LD ────────────────────────────────────────────

    #[test]
    fn redact_keeps_only_the_ends_of_a_full_iban() {
        // Enough to locate the value in front matter, not enough to be
        // an account number.
        assert_eq!(redact_for_log("GB29NWBK60161331926819"), "GB29…6819");
        assert_eq!(redact_for_log("BE68539007547034"), "BE68…7034");
    }

    #[test]
    fn redact_masks_short_values_entirely() {
        // A BIC is 8 or 11 characters. Splitting 4-and-4 would reveal
        // most of it, so anything at or under 12 is masked whole.
        assert_eq!(redact_for_log("NWBKGB2L"), "………");
        assert_eq!(redact_for_log("NWBKGB2LXXX"), "………");
        assert_eq!(redact_for_log(""), "………");
        assert_eq!(redact_for_log("GB29"), "………");
    }

    #[test]
    fn redact_never_returns_the_input_unchanged() {
        // The property that matters: whatever goes in, the full value
        // never comes back out.
        for value in [
            "GB29NWBK60161331926819",
            "BE68539007547034",
            "NWBKGB2L",
            "",
            "not-an-iban-but-long-enough",
        ] {
            assert_ne!(
                redact_for_log(value),
                value,
                "redaction returned {value} unchanged"
            );
        }
    }

    #[test]
    fn redact_handles_multibyte_input_without_panicking() {
        // Front matter is author-supplied; slicing by byte offset would
        // panic on a multi-byte scalar.
        let _ = redact_for_log("ééééééééééééééééé");
        let _ = redact_for_log("日本語のテキストです長いです");
        let _ = redact_for_log("🏦🏦🏦🏦🏦🏦🏦🏦🏦🏦🏦🏦🏦🏦");
    }

    #[test]
    fn bank_account_jsonld_includes_iban_and_bic_namespaced() {
        let acc = BankAccount {
            name: Some("Treasury".to_string()),
            iban: Some("GB29NWBK60161331926819".to_string()),
            bic: Some("NWBKGB2L".to_string()),
        };
        let v = acc.to_jsonld();
        assert_eq!(v["@type"], "BankAccount");
        assert_eq!(v["iso20022:iban"], "GB29NWBK60161331926819");
        assert_eq!(v["iso20022:bic"], "NWBKGB2L");
        assert_eq!(v["name"], "Treasury");
        // No generic `identifier` field — the IBAN is already the
        // typed identifier via the `iso20022:iban` namespace.
        // Duplicating the same string under `identifier` was
        // confusing for downstream consumers (and lit up in
        // examples/iso20022_example.rs output).
        assert!(v.get("identifier").is_none());
    }

    #[test]
    fn bank_account_jsonld_omits_optional_fields() {
        let v = BankAccount::default().to_jsonld();
        assert!(v.get("iso20022:iban").is_none());
        assert!(v.get("iso20022:bic").is_none());
        assert!(v.get("name").is_none());
    }

    #[test]
    fn payment_instrument_emits_namespaced_type() {
        let p = PaymentInstrument {
            name: Some("Visa Debit".to_string()),
            instrument_type: "card".to_string(),
            brand: Some("Visa".to_string()),
        };
        let v = p.to_jsonld();
        assert_eq!(v["@type"], "PaymentService");
        assert_eq!(v["iso20022:instrumentType"], "card");
        assert_eq!(v["brand"], "Visa");
    }

    #[test]
    fn financial_transaction_jsonld_shape_full() {
        let t = FinancialTransaction {
            instructed_amount: Some(MonetaryAmount {
                currency: "EUR".to_string(),
                amount: 1500.00,
            }),
            debtor_account: Some(BankAccount {
                name: None,
                iban: Some("GB29NWBK60161331926819".to_string()),
                bic: None,
            }),
            creditor_account: Some(BankAccount {
                name: None,
                iban: Some("DE89370400440532013000".to_string()),
                bic: None,
            }),
            execution_date: Some("2026-06-25".to_string()),
            end_to_end_id: Some("E2E-001".to_string()),
        };
        let v = t.to_jsonld();
        assert_eq!(v["@type"], "MoneyTransfer");
        assert_eq!(v["amount"]["currency"], "EUR");
        assert_eq!(v["amount"]["value"], 1500.0);
        assert_eq!(
            v["iso20022:debtorAccount"]["iso20022:iban"],
            "GB29NWBK60161331926819"
        );
        // Sub-objects must not carry a redundant @context.
        assert!(v["iso20022:debtorAccount"].get("@context").is_none());
        assert_eq!(v["iso20022:endToEndId"], "E2E-001");
        assert_eq!(v["identifier"], "E2E-001");
    }

    #[test]
    fn regulated_institution_jsonld_includes_lei() {
        let r = RegulatedFinancialInstitution {
            name: "Acme Bank plc".to_string(),
            lei: Some("529900W18LQJJN6SJ336".to_string()),
            licence_id: Some("FCA-FRN-123456".to_string()),
            regulator: Some("FCA".to_string()),
            url: Some("https://acme.example".to_string()),
        };
        let v = r.to_jsonld();
        assert_eq!(v["@type"], "BankOrCreditUnion");
        assert_eq!(v["name"], "Acme Bank plc");
        assert_eq!(v["iso20022:lei"], "529900W18LQJJN6SJ336");
        assert_eq!(v["iso20022:licenceId"], "FCA-FRN-123456");
    }

    #[test]
    fn financial_product_jsonld_includes_isin_and_apr() {
        let p = FinancialProduct {
            name: "Green Bond 2030".to_string(),
            product_type: "deposit".to_string(),
            issuer: Some("Acme Bank".to_string()),
            annual_percentage_rate: Some(3.5),
            isin: Some("US0378331005".to_string()),
        };
        let v = p.to_jsonld();
        assert_eq!(v["@type"], "FinancialProduct");
        assert_eq!(v["name"], "Green Bond 2030");
        assert_eq!(v["iso20022:productType"], "deposit");
        assert_eq!(v["iso20022:isin"], "US0378331005");
        assert_eq!(v["annualPercentageRate"], 3.5);
        assert_eq!(v["provider"]["@type"], "Organization");
        assert_eq!(v["provider"]["name"], "Acme Bank");
    }

    // ── Frontmatter dispatch ────────────────────────────────────────

    #[test]
    fn dispatch_bank_account_round_trip() {
        let fm = serde_json::json!({
            "type": "BankAccount",
            "iban": "GB29NWBK60161331926819",
        });
        let entity = from_frontmatter(&fm).unwrap();
        assert_eq!(entity.type_name(), "BankAccount");
        let jsonld = entity.to_jsonld();
        assert_eq!(jsonld["iso20022:iban"], "GB29NWBK60161331926819");
    }

    #[test]
    fn dispatch_financial_transaction_from_yaml_shape() {
        let fm = serde_json::json!({
            "type": "FinancialTransaction",
            "instructed_amount": {"currency": "EUR", "amount": 1500.00},
            "debtor_account": {"iban": "GB29NWBK60161331926819"},
            "creditor_account": {"iban": "DE89370400440532013000"},
        });
        let entity = from_frontmatter(&fm).unwrap();
        assert_eq!(entity.type_name(), "FinancialTransaction");
        let jsonld = entity.to_jsonld();
        assert_eq!(jsonld["amount"]["currency"], "EUR");
    }

    #[test]
    fn dispatch_missing_type_errors() {
        let fm = serde_json::json!({"iban": "GB29NWBK60161331926819"});
        let err = from_frontmatter(&fm).unwrap_err();
        assert_eq!(err, DispatchError::MissingType);
    }

    #[test]
    fn dispatch_unknown_type_errors() {
        let fm = serde_json::json!({"type": "GalacticCredits"});
        let err = from_frontmatter(&fm).unwrap_err();
        assert!(
            matches!(err, DispatchError::UnknownType(t) if t == "GalacticCredits")
        );
    }

    // ── Warning emission ────────────────────────────────────────────

    #[test]
    fn warn_invalid_fields_counts_iban_failure() {
        let entity = Iso20022Entity::BankAccount(BankAccount {
            iban: Some("INVALID-IBAN".to_string()),
            ..BankAccount::default()
        });
        let warnings = warn_invalid_fields(&entity, "page.md");
        assert_eq!(warnings, 1);
    }

    #[test]
    fn warn_invalid_fields_counts_zero_for_valid_iban() {
        let entity = Iso20022Entity::BankAccount(BankAccount {
            iban: Some("GB29NWBK60161331926819".to_string()),
            bic: Some("NWBKGB2L".to_string()),
            ..BankAccount::default()
        });
        let warnings = warn_invalid_fields(&entity, "page.md");
        assert_eq!(warnings, 0);
    }

    #[test]
    fn warn_invalid_fields_walks_transaction_subfields() {
        let entity =
            Iso20022Entity::FinancialTransaction(FinancialTransaction {
                debtor_account: Some(BankAccount {
                    iban: Some("BAD".to_string()),
                    ..BankAccount::default()
                }),
                creditor_account: Some(BankAccount {
                    bic: Some("BADBIC".to_string()), // length 6, invalid
                    ..BankAccount::default()
                }),
                ..FinancialTransaction::default()
            });
        let warnings = warn_invalid_fields(&entity, "page.md");
        assert_eq!(warnings, 2);
    }

    // ── First-use info pointer ──────────────────────────────────────

    #[test]
    fn first_use_pointer_fires_exactly_once() {
        reset_first_use_for_test();
        assert!(!first_use_was_logged());
        log_first_use_pointer();
        assert!(first_use_was_logged());
        // Calling again is a no-op (the flag stays set).
        log_first_use_pointer();
        assert!(first_use_was_logged());
    }

    // ── Schema.org subset validator ─────────────────────────────────

    #[test]
    fn schema_validator_passes_bank_account_with_identifier() {
        let v = BankAccount {
            iban: Some("GB29NWBK60161331926819".to_string()),
            ..BankAccount::default()
        }
        .to_jsonld();
        assert!(validate_schema_org(&v).is_empty());
    }

    #[test]
    fn schema_validator_flags_bank_account_without_identifier() {
        let v = BankAccount::default().to_jsonld();
        let errs = validate_schema_org(&v);
        assert!(errs.iter().any(|e| e.field.contains("identifier")));
    }

    #[test]
    fn schema_validator_passes_money_transfer_with_amount() {
        let v = FinancialTransaction {
            instructed_amount: Some(MonetaryAmount {
                currency: "USD".to_string(),
                amount: 10.0,
            }),
            ..FinancialTransaction::default()
        }
        .to_jsonld();
        assert!(validate_schema_org(&v).is_empty());
    }

    #[test]
    fn schema_validator_flags_money_transfer_missing_amount() {
        let v = FinancialTransaction::default().to_jsonld();
        let errs = validate_schema_org(&v);
        assert!(errs.iter().any(|e| e.field == "amount"));
    }

    #[test]
    fn schema_validator_flags_empty_institution_name() {
        let v = RegulatedFinancialInstitution {
            name: String::new(),
            ..RegulatedFinancialInstitution::default()
        }
        .to_jsonld();
        let errs = validate_schema_org(&v);
        assert!(errs.iter().any(|e| e.field == "name"));
    }

    #[test]
    fn schema_validator_flags_missing_context() {
        let v = serde_json::json!({"@type": "BankAccount", "identifier": "x"});
        let errs = validate_schema_org(&v);
        assert!(errs.iter().any(|e| e.field == "@context"));
    }

    // ── Optional-field omission in JSON-LD emitters ─────────────────

    #[test]
    fn payment_instrument_jsonld_omits_optional_fields() {
        let p = PaymentInstrument {
            name: None,
            instrument_type: "transfer".to_string(),
            brand: None,
        };
        let v = p.to_jsonld();
        assert!(v.get("name").is_none());
        assert!(v.get("brand").is_none());
        assert_eq!(v["iso20022:instrumentType"], "transfer");
    }

    #[test]
    fn financial_product_jsonld_omits_optional_fields() {
        let p = FinancialProduct {
            name: "Plain Loan".to_string(),
            product_type: "loan".to_string(),
            issuer: None,
            annual_percentage_rate: None,
            isin: None,
        };
        let v = p.to_jsonld();
        assert!(v.get("provider").is_none());
        assert!(v.get("annualPercentageRate").is_none());
        assert!(v.get("iso20022:isin").is_none());
        assert!(v.get("identifier").is_none());
    }

    #[test]
    fn strip_context_passes_non_object_values_through() {
        let v = strip_context(serde_json::json!("scalar"));
        assert_eq!(v, serde_json::json!("scalar"));
    }

    // ── Iso20022Entity dispatch surfaces ────────────────────────────

    #[test]
    fn entity_to_jsonld_covers_remaining_variants() {
        let pi = Iso20022Entity::PaymentInstrument(PaymentInstrument {
            instrument_type: "card".to_string(),
            ..PaymentInstrument::default()
        });
        assert_eq!(pi.to_jsonld()["@type"], "PaymentService");

        let rfi = Iso20022Entity::RegulatedFinancialInstitution(
            RegulatedFinancialInstitution {
                name: "Acme Bank".to_string(),
                ..RegulatedFinancialInstitution::default()
            },
        );
        assert_eq!(rfi.to_jsonld()["@type"], "BankOrCreditUnion");

        let fp = Iso20022Entity::FinancialProduct(FinancialProduct {
            name: "Bond".to_string(),
            product_type: "derivative".to_string(),
            ..FinancialProduct::default()
        });
        assert_eq!(fp.to_jsonld()["@type"], "FinancialProduct");
    }

    #[test]
    fn entity_type_name_payment_instrument() {
        let pi =
            Iso20022Entity::PaymentInstrument(PaymentInstrument::default());
        assert_eq!(pi.type_name(), "PaymentInstrument");
    }

    // ── DispatchError display ───────────────────────────────────────

    #[test]
    fn dispatch_error_display_all_variants() {
        assert!(DispatchError::MissingType
            .to_string()
            .contains("missing the required `type` field"));
        assert!(DispatchError::UnknownType("Widget".to_string())
            .to_string()
            .contains("`Widget` is not one of"));
        assert!(DispatchError::Malformed("bad shape".to_string())
            .to_string()
            .contains("malformed: bad shape"));
    }

    // ── from_frontmatter: remaining discriminants + error paths ────

    #[test]
    fn dispatch_non_string_type_field_is_missing_type() {
        let fm = serde_json::json!({"type": 42});
        assert_eq!(
            from_frontmatter(&fm).unwrap_err(),
            DispatchError::MissingType
        );
    }

    #[test]
    fn dispatch_payment_instrument_round_trip_and_malformed() {
        let ok = serde_json::json!({
            "type": "PaymentInstrument",
            "instrument_type": "card",
        });
        let entity = from_frontmatter(&ok).unwrap();
        assert_eq!(entity.type_name(), "PaymentInstrument");

        // `instrument_type` is required — omitting it fails deserialise.
        let bad = serde_json::json!({"type": "PaymentInstrument"});
        assert!(matches!(
            from_frontmatter(&bad),
            Err(DispatchError::Malformed(_))
        ));
    }

    #[test]
    fn dispatch_regulated_institution_round_trip_and_malformed() {
        let ok = serde_json::json!({
            "type": "RegulatedFinancialInstitution",
            "name": "Acme Bank",
        });
        let entity = from_frontmatter(&ok).unwrap();
        assert_eq!(entity.type_name(), "RegulatedFinancialInstitution");

        let bad = serde_json::json!({"type": "RegulatedFinancialInstitution"});
        assert!(matches!(
            from_frontmatter(&bad),
            Err(DispatchError::Malformed(_))
        ));
    }

    #[test]
    fn dispatch_financial_product_round_trip_and_malformed() {
        let ok = serde_json::json!({
            "type": "FinancialProduct",
            "name": "Green Bond",
            "product_type": "deposit",
        });
        let entity = from_frontmatter(&ok).unwrap();
        assert_eq!(entity.type_name(), "FinancialProduct");

        let bad = serde_json::json!({"type": "FinancialProduct"});
        assert!(matches!(
            from_frontmatter(&bad),
            Err(DispatchError::Malformed(_))
        ));
    }

    #[test]
    fn dispatch_bank_account_malformed_payload() {
        // `iban` must be a string — a number fails deserialisation.
        let bad = serde_json::json!({"type": "BankAccount", "iban": 123});
        assert!(matches!(
            from_frontmatter(&bad),
            Err(DispatchError::Malformed(_))
        ));
    }

    #[test]
    fn dispatch_financial_transaction_malformed_payload() {
        let bad = serde_json::json!({
            "type": "FinancialTransaction",
            "debtor_account": "not an object",
        });
        assert!(matches!(
            from_frontmatter(&bad),
            Err(DispatchError::Malformed(_))
        ));
    }

    // ── warn_invalid_fields: remaining walk combinations ────────────

    #[test]
    fn warn_walks_bank_account_with_bic_only() {
        let e = Iso20022Entity::BankAccount(BankAccount {
            iban: None,
            bic: Some("BAD".to_string()),
            ..BankAccount::default()
        });
        assert_eq!(warn_invalid_fields(&e, "page.md"), 1);
    }

    #[test]
    fn warn_walks_transaction_with_sparse_accounts() {
        // Debtor carries only a BIC; creditor carries only an IBAN —
        // exercises every Some/None combination in the account walk.
        let e = Iso20022Entity::FinancialTransaction(FinancialTransaction {
            debtor_account: Some(BankAccount {
                iban: None,
                bic: Some("BAD".to_string()),
                ..BankAccount::default()
            }),
            creditor_account: Some(BankAccount {
                iban: Some("INVALID".to_string()),
                bic: None,
                ..BankAccount::default()
            }),
            ..FinancialTransaction::default()
        });
        assert_eq!(warn_invalid_fields(&e, "page.md"), 2);
    }

    #[test]
    fn warn_transaction_without_accounts_emits_nothing() {
        let e = Iso20022Entity::FinancialTransaction(
            FinancialTransaction::default(),
        );
        assert_eq!(warn_invalid_fields(&e, "page.md"), 0);
    }

    #[test]
    fn warn_skips_entities_without_account_fields() {
        let pi =
            Iso20022Entity::PaymentInstrument(PaymentInstrument::default());
        assert_eq!(warn_invalid_fields(&pi, "page.md"), 0);
        let fp = Iso20022Entity::FinancialProduct(FinancialProduct::default());
        assert_eq!(warn_invalid_fields(&fp, "page.md"), 0);
    }

    // ── Schema.org validator: remaining arms ────────────────────────

    #[test]
    fn schema_validator_flags_financial_product_missing_name() {
        // Hits the second literal of the BankOrCreditUnion |
        // FinancialProduct match arm.
        let v = serde_json::json!({
            "@context": "https://schema.org",
            "@type": "FinancialProduct",
        });
        let errs = validate_schema_org(&v);
        assert!(errs.iter().any(|e| e.field == "name"));
    }

    #[test]
    fn schema_validator_passes_institution_with_name() {
        let v = RegulatedFinancialInstitution {
            name: "Acme Bank".to_string(),
            ..RegulatedFinancialInstitution::default()
        }
        .to_jsonld();
        assert!(validate_schema_org(&v).is_empty());
    }

    #[test]
    fn schema_validator_flags_payment_service_missing_instrument_type() {
        let v = serde_json::json!({
            "@context": "https://schema.org",
            "@type": "PaymentService",
        });
        let errs = validate_schema_org(&v);
        assert!(errs.iter().any(|e| e.field == "iso20022:instrumentType"));
    }

    #[test]
    fn schema_validator_passes_payment_service_with_instrument_type() {
        let v = PaymentInstrument {
            instrument_type: "card".to_string(),
            ..PaymentInstrument::default()
        }
        .to_jsonld();
        assert!(validate_schema_org(&v).is_empty());
    }

    #[test]
    fn schema_validator_ignores_unknown_types() {
        let v = serde_json::json!({
            "@context": "https://schema.org",
            "@type": "SomethingElse",
        });
        assert!(validate_schema_org(&v).is_empty());
    }

    #[test]
    fn schema_validator_defaults_to_unknown_when_type_field_absent() {
        // Distinct from `..._ignores_unknown_types` above: here `@type`
        // is missing entirely, exercising the `.unwrap_or("Unknown")`
        // fallback (as opposed to an unrecognised *value*).
        let v = serde_json::json!({"@context": "https://schema.org"});
        let errs = validate_schema_org(&v);
        assert!(errs.is_empty(), "Unknown type enforces no required fields");
    }

    // ── Derived PartialEq/Eq surfaces ───────────────────────────────
    //
    // These domain types are rarely compared via `==` in the tests
    // above (assertions mostly poke at the rendered JSON), so the
    // derived `eq` impls need their own direct exercise, including a
    // difference in a *later* field so short-circuiting `&&` chains
    // don't leave the tail comparisons unexecuted.

    #[test]
    fn domain_types_partial_eq_covers_equal_and_unequal_tail_fields() {
        let a = MonetaryAmount {
            currency: "EUR".to_string(),
            amount: 1.0,
        };
        let b = MonetaryAmount {
            currency: "EUR".to_string(),
            amount: 1.0,
        };
        let c = MonetaryAmount {
            currency: "EUR".to_string(),
            amount: 2.0,
        };
        assert_eq!(a, b);
        assert_ne!(a, c);

        let ba1 = BankAccount {
            name: Some("N".to_string()),
            iban: Some("I".to_string()),
            bic: Some("B".to_string()),
        };
        let ba2 = ba1.clone();
        let mut ba3 = ba1.clone();
        ba3.bic = Some("DIFFERENT".to_string());
        assert_eq!(ba1, ba2);
        assert_ne!(ba1, ba3);

        let pi1 = PaymentInstrument {
            name: Some("N".to_string()),
            instrument_type: "card".to_string(),
            brand: Some("Visa".to_string()),
        };
        let mut pi2 = pi1.clone();
        pi2.brand = Some("Other".to_string());
        assert_eq!(pi1, pi1.clone());
        assert_ne!(pi1, pi2);

        let rfi1 = RegulatedFinancialInstitution {
            name: "Acme".to_string(),
            lei: Some("L".to_string()),
            licence_id: Some("LIC".to_string()),
            regulator: Some("FCA".to_string()),
            url: Some("https://x".to_string()),
        };
        let mut rfi2 = rfi1.clone();
        rfi2.url = Some("https://y".to_string());
        assert_eq!(rfi1, rfi1.clone());
        assert_ne!(rfi1, rfi2);

        let fp1 = FinancialProduct {
            name: "Bond".to_string(),
            product_type: "deposit".to_string(),
            issuer: Some("Acme".to_string()),
            annual_percentage_rate: Some(1.0),
            isin: Some("US1".to_string()),
        };
        let mut fp2 = fp1.clone();
        fp2.isin = Some("US2".to_string());
        assert_eq!(fp1, fp1.clone());
        assert_ne!(fp1, fp2);

        let ft1 = FinancialTransaction {
            instructed_amount: Some(a.clone()),
            debtor_account: Some(ba1.clone()),
            creditor_account: Some(ba1.clone()),
            execution_date: Some("2026-01-01".to_string()),
            end_to_end_id: Some("E1".to_string()),
        };
        let mut ft2 = ft1.clone();
        ft2.end_to_end_id = Some("E2".to_string());
        assert_eq!(ft1, ft1.clone());
        assert_ne!(ft1, ft2);

        let se1 = SchemaOrgError {
            schema_type: "BankAccount".to_string(),
            field: "identifier".to_string(),
            reason: "missing".to_string(),
        };
        let mut se2 = se1.clone();
        se2.reason = "different".to_string();
        assert_eq!(se1, se1.clone());
        assert_ne!(se1, se2);

        let vo1 = ValidationOutcome::Invalid {
            reason: "x".to_string(),
        };
        let vo2 = ValidationOutcome::Invalid {
            reason: "y".to_string(),
        };
        assert_eq!(
            vo1,
            ValidationOutcome::Invalid {
                reason: "x".to_string()
            }
        );
        assert_ne!(vo1, vo2);
        assert_ne!(vo1, ValidationOutcome::Valid);

        let e1 = Iso20022Entity::FinancialProduct(fp1.clone());
        let e2 = Iso20022Entity::FinancialProduct(fp2.clone());
        assert_eq!(e1, Iso20022Entity::FinancialProduct(fp1.clone()));
        assert_ne!(e1, e2);
        assert_ne!(e1, Iso20022Entity::BankAccount(ba1.clone()));
    }
}
