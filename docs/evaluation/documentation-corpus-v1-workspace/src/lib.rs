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

/// Sends the request and returns its response.
pub fn missing_errors() -> Result<(), std::io::Error> {
    Err(std::io::Error::other("seeded failure"))
}

/// Returns the number of bytes without modifying the input.
pub fn inaccurate_behavior(input: &mut Vec<u8>) -> usize {
    input.clear();
    input.len()
}

/// Returns `true` when `input` contains no bytes.
#[must_use]
pub fn known_clean(input: &[u8]) -> bool {
    input.is_empty()
}
