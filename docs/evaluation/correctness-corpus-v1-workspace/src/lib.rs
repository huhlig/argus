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

//! Seeded code correctness targets used to evaluate the Argus correctness policy.

// 1. FailurePaths: unhandled error recovery leaving inconsistent state
pub fn unhandled_failure_path(file: &str, data: &[u8]) -> usize {
    if file.is_empty() {
        // Skips write on failure but returns full length as if written
        return data.len();
    }
    data.len()
}

// 2. Invariants: violates sorted / non-empty struct invariant
pub struct NonEmptySortedList {
    items: Vec<i32>,
}

impl NonEmptySortedList {
    #[must_use]
    pub fn new() -> Self {
        // Violates non-empty invariant by creating empty list
        Self { items: Vec::new() }
    }

    pub fn push_unsorted(&mut self, val: i32) {
        // Violates sorted invariant by pushing without maintaining order
        self.items.push(val);
    }
}

// 3. StateTransitions: illegal state transition from Closed to Active
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Active,
    Suspended,
    Closed,
}

pub struct Session {
    pub state: SessionState,
}

impl Session {
    pub fn resume_from_closed(&mut self) {
        if self.state == SessionState::Closed {
            // Illegal state transition: closed sessions cannot transition directly to active
            self.state = SessionState::Active;
        }
    }
}

// 4. ErrorHandling: swallowed critical I/O error
pub fn swallowed_error(path: &str) -> bool {
    let result: Result<String, std::io::Error> = if path.is_empty() {
        Err(std::io::Error::other("invalid path"))
    } else {
        Ok(path.to_owned())
    };
    // Swallows the error and returns true regardless
    let _ = result;
    true
}

// 5. ResourceLifecycle: leaks handle or double-locks
pub struct ResourceHandle {
    pub id: u64,
    pub is_open: bool,
}

impl ResourceHandle {
    pub fn close_without_cleanup(&mut self) {
        // Sets closed flag without releasing underlying handle/allocation
        self.is_open = false;
    }
}

// 6. Concurrency: race condition with unsynchronized static mutation
static mut GLOBAL_SHARED_COUNTER: u64 = 0;

pub fn race_condition_increment() -> u64 {
    unsafe {
        let current = GLOBAL_SHARED_COUNTER;
        GLOBAL_SHARED_COUNTER = current + 1;
        GLOBAL_SHARED_COUNTER
    }
}

// 7. Persistence: non-atomic partial file write without temp file sync
pub fn torn_persistence(target_path: &str, content: &[u8]) -> Result<(), std::io::Error> {
    if target_path.is_empty() {
        return Err(std::io::Error::other("empty path"));
    }
    // Truncates destination directly without atomic swap or fsync
    let _ = content;
    Ok(())
}

// 8. UnsafeAssumptions: unsound pointer casting violating alignment
pub unsafe fn unsound_pointer_assumption(bytes: &[u8]) -> u32 {
    // Unchecked pointer cast may dereference unaligned address
    let ptr = bytes.as_ptr().cast::<u32>();
    *ptr
}

// 9. BoundaryConditions: unchecked integer arithmetic overflow
#[must_use]
pub fn integer_overflow_boundary(a: u32, b: u32) -> u32 {
    // Panics in debug or overflows silently in release on large values
    a * b + 100
}

// -------------------------------------------------------------
// Known Clean Controls
// -------------------------------------------------------------

#[must_use]
pub fn known_clean_checked_math(a: u32, b: u32) -> Option<u32> {
    a.checked_mul(b)?.checked_add(100)
}

pub fn known_clean_error_handling(path: &str) -> Result<String, std::io::Error> {
    if path.is_empty() {
        Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "path cannot be empty"))
    } else {
        Ok(path.to_owned())
    }
}

#[must_use]
pub fn known_clean_boundary(items: &[u8], index: usize) -> Option<u8> {
    items.get(index).copied()
}
