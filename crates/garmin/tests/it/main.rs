//! Integration tests of `gymgar-garmin` that read the scrubbed fixtures in `fixtures/garmin/`.

// Test helpers outside `#[test]` fns are not test code to clippy, and a failed expectation is how these tests fail.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod fit_fixture;
