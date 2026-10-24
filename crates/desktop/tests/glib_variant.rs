//! Wiki: docs/wiki/glib-backport.md

#![cfg(target_os = "linux")]
#![forbid(unsafe_code)]
#![deny(warnings, rust_2018_idioms, rust_2021_compatibility)]
#![warn(missing_docs, missing_debug_implementations)]

use glib::variant::{ToVariant, VariantTypeMismatchError};

#[test]
fn string_variant_iteration_preserves_values() -> Result<(), VariantTypeMismatchError> {
    let values = ["bridge", "", "λ", "device"];
    let variant = values.to_variant();

    assert_eq!(variant.array_iter_str()?.collect::<Vec<_>>(), values);
    assert_eq!(
        variant.array_iter_str()?.rev().collect::<Vec<_>>(),
        ["device", "λ", "", "bridge"]
    );
    assert_eq!(variant.array_iter_str()?.nth(2), Some("λ"));
    assert_eq!(variant.array_iter_str()?.nth_back(2), Some(""));
    assert_eq!(variant.array_iter_str()?.last(), Some("device"));

    let mut mixed = variant.array_iter_str()?;
    assert_eq!(mixed.next(), Some("bridge"));
    assert_eq!(mixed.next_back(), Some("device"));
    assert_eq!(mixed.next(), Some(""));
    assert_eq!(mixed.next_back(), Some("λ"));
    assert_eq!(mixed.next(), None);
    assert_eq!(mixed.next_back(), None);

    let empty: [&str; 0] = [];
    assert_eq!(empty.to_variant().array_iter_str()?.next(), None);
    Ok(())
}
