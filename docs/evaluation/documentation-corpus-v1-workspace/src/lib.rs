// Copyright 2026 Hans W. Uhlig
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Seeded public API documentation used to evaluate the Argus documentation policy.

// 1. Presence: completely missing documentation
#[must_use]
pub fn missing_presence(x: u32) -> u32 {
    x.saturating_add(1)
}

// 2. Purpose: vague/unclear intent
/// Helper function.
#[must_use]
pub fn unclear_purpose(items: &[u8]) -> usize {
    items.len()
}

// 3. Behavior: omits key transformation / operational behavior
/// Processes the bytes.
#[must_use]
pub fn missing_behavior(input: &[u8]) -> Vec<u8> {
    input.iter().copied().filter(|b| b % 2 == 0).collect()
}

// 4. Inputs: multiple parameters with omitted explanation
/// Computes an offset within the buffer.
#[must_use]
pub fn missing_inputs(buffer_len: usize, index: usize, stride: usize) -> usize {
    (index * stride).min(buffer_len)
}

// 5. Outputs: non-obvious return structure omitted
/// Evaluates telemetry and outputs status.
#[must_use]
pub fn missing_outputs(rate: f64) -> (bool, u32) {
    (rate > 0.5, (rate * 100.0) as u32)
}

// 6. Errors: returns Result without documenting error conditions
/// Sends the request and returns its response.
pub fn missing_errors() -> Result<(), std::io::Error> {
    Err(std::io::Error::other("seeded failure"))
}

// 7. Panics: has explicit panic conditions without # Panics
/// Divides `numerator` by `denominator`.
#[must_use]
pub fn missing_panics(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        panic!("denominator must not be zero");
    }
    numerator / denominator
}

// 8. Safety: unsafe fn without # Safety section
/// Dereferences the raw pointer.
///
/// # Returns
///
/// The byte value.
#[must_use]
pub unsafe fn missing_safety(ptr: *const u8) -> u8 {
    *ptr
}

// 9. SideEffects: modifies external / static state without doc
static mut COUNTER: u64 = 0;

/// Returns the current generation number.
#[must_use]
pub fn undocumented_side_effects() -> u64 {
    unsafe {
        COUNTER += 1;
        COUNTER
    }
}

// 10. Invariants: struct with strict invariant omitted from docs
/// A bounded non-empty byte slice wrapper.
pub struct UndocumentedInvariants {
    /// Inner payload.
    pub data: Vec<u8>,
}

// 11. Examples: contains contradictory or misleading code examples
/// Doubles the input number.
///
/// ```
/// let res = 100;
/// assert_eq!(res, 100);
/// ```
#[must_use]
pub fn misleading_examples(x: i32) -> i32 {
    x * 2
}

// 12. Accuracy: claims something contradicted by implementation
/// Returns the number of bytes without modifying the input.
pub fn inaccurate_behavior(input: &mut Vec<u8>) -> usize {
    input.clear();
    input.len()
}

// 13. Currency: refers to obsolete/renamed arguments
/// Parses the packet given `payload_length` and timeout.
#[must_use]
pub fn obsolete_currency(bytes: &[u8]) -> usize {
    bytes.len()
}

// 14. Value: vacuous tautology that adds zero semantic insight
/// Performs computation.
#[must_use]
pub fn vacuous_tautology(val: u32) -> u32 {
    val.rotate_left(3)
}

// -------------------------------------------------------------
// Known Clean Controls
// -------------------------------------------------------------

/// Returns `true` when `input` contains no bytes.
#[must_use]
pub fn known_clean(input: &[u8]) -> bool {
    input.is_empty()
}

/// Reads the byte from `ptr`.
///
/// # Safety
///
/// `ptr` must be non-null and valid for reads of 1 byte.
#[must_use]
pub unsafe fn known_clean_unsafe(ptr: *const u8) -> u8 {
    *ptr
}

/// Opens and validates the configuration file.
///
/// # Errors
///
/// Returns an [`std::io::Error`] if the path is empty.
pub fn known_clean_error(path: &str) -> Result<String, std::io::Error> {
    if path.is_empty() {
        Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty path"))
    } else {
        Ok(path.to_owned())
    }
}
